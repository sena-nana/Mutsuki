# Handle 与 RefPayload

## 这是什么

MutsukiBot 的"按引用"协议：

- `Handle[T]` 是一个**引用计数**的所有权抽象，持有不可序列化对象（GPU 张量、KV 缓存槽、模型权重……）
- `RefPayload[T]` 是契约层标记，表明某字段通过引用持有（携带 handle + descriptor）
- `RefDescriptor` 是引用的**可观测元数据**——永远可序列化，写到 trace / audit 里就用它

代码：

- 协议：[mutsukibot/contracts/refpayload.py](../../mutsukibot/contracts/refpayload.py)
- 默认实现：[mutsukibot/core/handle.py](../../mutsukibot/core/handle.py)

## 解决什么问题

Yume / mind-sim 这种系统会传递 latent 张量、KV 缓存这类**不能被 msgspec 序列化的对象**。如果把它们当普通 Struct 字段处理，要么：

1. 强制 in-process 执行（不能用 service 解耦）
2. 每次跨插件就拷贝（GPU 显存炸）

MutsukiBot 的方案：把这种对象的"字段位置"用 `RefPayload[T]` 标记。codec / trace / audit 看到这个字段时，知道**只能落 descriptor，不能落 handle 本身**。运行时仍然用 handle 直接共享对象 —— 但在跨进程边界、序列化时会被显式拒绝。

同时 handle 的引用计数 + scope 集成保证：用完不释放 → 卸载插件时 scope 报泄漏。

## 怎么工作

### Handle 抽象基类

[contracts/refpayload.py:49-82](../../mutsukibot/contracts/refpayload.py#L49-L82)：

```python
class Handle(ABC, Generic[T]):
    @abstractmethod
    def acquire(self) -> T: ...
    @abstractmethod
    def release(self) -> None: ...
    @abstractmethod
    def borrow(self) -> AbstractContextManager[T]: ...
    @abstractmethod
    def is_alive(self) -> bool: ...
    @abstractmethod
    def attach_to(self, scope: "PluginScope | TransactionScope") -> None: ...

    @property @abstractmethod
    def ref_id(self) -> RefId: ...
    @property @abstractmethod
    def descriptor(self) -> RefDescriptor: ...
```

### RefDescriptor：永远可序列化的元数据

[contracts/refpayload.py:35-46](../../mutsukibot/contracts/refpayload.py#L35-L46)：

```python
class RefDescriptor(Contract):
    ref_id: RefId
    kind: str                                      # 领域类别（如 "yume.latent"）
    schema_id_target: str                          # 持有什么类型（如 "yume.latent/v2"）
    schema_version_target: str                     # SemVer
    attributes: dict[str, str | int | float | bool] = {}
    lineage: tuple[RefId, ...] = ()                # 派生链
```

`attributes` 由领域决定（如 `{"shape": "1024,768", "dtype": "fp16"}`）。trace 写入只保留 descriptor，不会保留 target —— 这是按设计：观察者拿不到敏感对象本体，只拿元数据。

### RefCountedHandle：默认实现

[handle.py:56-134](../../mutsukibot/core/handle.py#L56-L134)。关键不变量：

1. **构造时 refcount = 1**（[handle.py:75](../../mutsukibot/core/handle.py#L75)）—— 这一份代表"调用者持有"
2. **第一次 `acquire` / `borrow` 之前必须 `attach_to(scope)`**（[handle.py:91-99](../../mutsukibot/core/handle.py#L91-L99) 的 `_check_attached`）—— 未 attach 即抛 `HandleNotAttachedError`，对应 `Errs.HANDLE_LEAK`
3. **`acquire` 时**：refcount += 1，返回 target
4. **`release` 时**：refcount -= 1；归零则调 finalizer 并标 released
5. **使用已释放 handle**：抛 `HandleUseAfterReleaseError`，对应 `Errs.HANDLE_USE_AFTER_RELEASE`

`borrow()` 是上下文管理器糖（[handle.py:125-131](../../mutsukibot/core/handle.py#L125-L131)）：

```python
@contextmanager
def borrow(self) -> "Generator[T]":
    target = self.acquire()
    try:
        yield target
    finally:
        self.release()
```

### Scope 集成

`attach_to(scope)` 把 handle 登记到 scope 的 handle 列表（[scope.py:87-89](../../mutsukibot/core/scope.py#L87-L89)）。scope.close 流程（[scope.py:127-143](../../mutsukibot/core/scope.py#L127-L143)）：

```python
for handle in self._state.handles:
    if not handle.is_alive():
        continue
    try:
        handle.release()
    except Exception as exc:
        cleanup_failures.append(...)
    if handle.is_alive():
        leaked.append(handle.ref_id)
```

注意：scope 调用一次 `release()`——抵消构造时的那个 1。如果 release 后 handle 仍 alive，意味着第三方仍持有（acquire 但未 release），就是泄漏，进 `leaked` list。

### HandleImpl 自动注册

[handle.py:42-53](../../mutsukibot/core/handle.py#L42-L53) 的 `HandleImpl` 是 `Handle[T]` 的具体实现基类，子类通过 `__init_subclass__` 自动登记到 `HandleRegistry`：

```python
class HandleImpl(Handle[T], Generic[T]):
    handle_kind: str = "generic"

    def __init_subclass__(cls, **kwargs: object) -> None:
        super().__init_subclass__(**kwargs)
        HandleRegistry.register(cls.handle_kind, cls)
```

领域插件可以子类化以提供自定义 finalizer（如归还显存槽）：

```python
class VramHandle(RefCountedHandle[Tensor]):
    handle_kind = "yume.vram"
    # 重写 finalizer 或 __init__，按需要做后端释放
```

### make_stub_handle：测试辅助

[handle.py:137-157](../../mutsukibot/core/handle.py#L137-L157) 的工厂用于测试：

```python
def make_stub_handle(
    ref_id: RefId,
    *,
    kind: str = "test.stub",
    schema_id_target: str = "test.stub/v1",
    schema_version_target: str = "1.0.0",
    target: object = None,
    attributes: dict | None = None,
) -> RefCountedHandle[object]:
    ...
```

让你在没有真实后端的情况下构造可观测的假 handle。

### RefPayload：契约层的字段标记

[contracts/refpayload.py:85-101](../../mutsukibot/contracts/refpayload.py#L85-L101)：

```python
class RefPayload(Contract, Generic[T]):
    ref_id: RefId
    handle: Handle[Any] = msgspec.field()
    descriptor: RefDescriptor
```

任何契约把它当字段类型，等于在告诉 codec："这里持有一个 handle，序列化你必须二选一 —— 拒绝（`Errs.REF_SERIALIZE_ATTEMPT`），或者降级成 descriptor"。v0.1 还没有 codec 拦截层，但契约的形状已经锁定。

## 用法示例

构造 handle 并绑定 scope：

```python
from mutsukibot.contracts.ids import RefId
from mutsukibot.contracts.refpayload import RefDescriptor
from mutsukibot.core.handle import RefCountedHandle

@command()
async def make_thing(self, ctx: AgentContext) -> str:
    descriptor = RefDescriptor(
        ref_id=RefId(ctx.id_gen.next("ref")),
        kind="my.thing",
        schema_id_target="my.thing/v1",
        schema_version_target="1.0.0",
        attributes={"size_kb": 128},
    )
    handle = RefCountedHandle(target=expensive_resource(), descriptor=descriptor)
    handle.attach_to(ctx.scope)
    self.services.register(MyHandleStore, handle)  # 让别的命令找得到
    return descriptor.ref_id
```

借用：

```python
@command()
async def use_thing(self, store: MyHandleStore = Inject()) -> str:
    handle = store.find()
    with handle.borrow() as target:
        return process(target)
    # 退出 with 时自动 release
```

测试用 stub handle 验证泄漏行为：

```python
from mutsukibot.contracts.ids import RefId
from mutsukibot.core.handle import make_stub_handle
from mutsukibot.core.scope import HandleLeakError, PluginScope

scope = PluginScope("test")
h = make_stub_handle(RefId("test_001"))
h.attach_to(scope)

# 故意多 acquire 一次不 release
_ = h.acquire()

try:
    await scope.close()
except HandleLeakError as e:
    assert "test_001" in str(e.leaked)
```

## 常见陷阱

- **未 `attach_to(scope)` 直接 `acquire`** → `HandleNotAttachedError`。这是 contracts §11.2 的硬规则。
- **`release` 多调一次没事，调 acquire 之前 release 也没事**——已 released 的 handle 再 release 是 no-op（[handle.py:114-116](../../mutsukibot/core/handle.py#L114-L116)）。但 `acquire` released handle 会抛 `HandleUseAfterReleaseError`。
- **scope.close 不知道你"想 acquire 几次"**。它只会 release 一次（抵消构造时的 1）。如果你 acquire 两次只 release 一次，差额会被报告为泄漏。
- **`RefDescriptor.attributes` 也只接受标量**——和 `Error.evidence` 一致。要塞结构化数据 → JSON 字符串。
- **`HandleRegistry` 按 `handle_kind` 索引**。如果两个子类都不改 `handle_kind`，第二个会覆盖第一个 —— 子类总是要重新声明 `handle_kind`。
- **`Handle[T]` 的 T 在运行时无效**。它是泛型形参；调度器和注册表都不感知 T 的具体类型。靠 `RefDescriptor.kind` / `schema_id_target` 做语义检查。
- **不要把 handle 当普通对象塞进消息字段**。Message 是 `msgspec.Struct`，msgspec 序列化时会试图遍历 handle —— 在 v0.1 没有 codec 守卫的情况下行为是未定义。要传引用就用 `RefPayload`。
