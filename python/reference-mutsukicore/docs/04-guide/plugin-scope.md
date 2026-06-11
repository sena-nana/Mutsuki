# PluginScope 与资源回收

## 这是什么

`PluginScope` 是一个插件实例（或一次事务）持有的所有副作用资源的**清单**。订阅、定时器、服务注册、context 挂件、配置监听、句柄（`Handle`）—— 都登记到 scope。卸载时 scope 反向调用所有清理函数；任何在关闭时仍存活的句柄被报告为泄漏。

代码：[mutsukicore/core/scope.py](../../mutsukicore/core/scope.py)。

## 解决什么问题

MutsukiCore 的 hard rule #4：**无副作用热重载**。卸载插件后，必须没有它残留的 task / 订阅 / GPU 显存 / 连接。这是把 Yume / mind-sim 装载到长寿命 agent 里的前提 —— 出了 bug 要能整段换掉而不是重启进程。

实现这一点有两种思路：

1. **依赖插件作者自觉清理**（NoneBot v1 的做法）。失败模式：忘了。
2. **强制副作用过 scope**，scope 集中清理。失败模式可观测：泄漏会在 `scope.close()` 时显式报错。

MutsukiCore 选 (2)。

## 怎么工作

### 五种登记口 + 一个句柄槽

[scope.py:65-89](../../mutsukicore/core/scope.py#L65-L89)：

```python
class PluginScope:
    def add_subscription(self, unsubscribe: CleanupFn) -> None
    def add_timer(self, cancel: CleanupFn) -> None
    def add_service_registration(self, unregister: CleanupFn) -> None
    def add_context_attachment(self, detach: CleanupFn) -> None
    def add_config_watcher(self, unwatch: CleanupFn) -> None
    def attach_handle(self, handle: Handle[object]) -> None
```

前五个接受一个清理回调（同步或 async 都行）；`attach_handle` 接受一个 `Handle`，`close()` 时会自动 release。

按 [ResourceKind](../../mutsukicore/core/scope.py#L29-L41) 区分种类只是为了**诊断**——`HandleLeakError.evidence` 里要告诉你"是哪类资源没清理"。运行时所有清理函数共用一条反向释放循环（[scope.py:110-122](../../mutsukicore/core/scope.py#L110-L122)）。

### close() 的执行顺序

[scope.py:103-167](../../mutsukicore/core/scope.py#L103-L167)：

1. 设置 `closed = True`（`_guard` 之后会拒绝新登记）
2. **反向**遍历 cleanups（LIFO，对称于 acquire 顺序）
3. 每个 cleanup 用 try / except 包住；失败收集到 `cleanup_failures` 列表，不打断后续清理
4. 遍历所有 attached handle：
   - 还活着的就 `release()`
   - release 仍然活着 → 进入 `leaked` 列表
5. 如果有 leaked 或 cleanup_failures → 构造 `Error(code=Errs.HANDLE_LEAK, ...)` 并抛 `HandleLeakError`

为什么 cleanup 失败不静默：[hard rule #8](../../AGENTS.md) "结构化错误，不允许吞异常返默认值"。诊断证据全部塞进 `error.evidence` 里（`cleanup_failures_json` 字段是数组的 JSON 序列化字符串，因为 `Error.evidence` 类型只允许标量）。

### 为什么 release 后还活着叫泄漏

`RefCountedHandle` 构造时引用计数 = 1（[handle.py:75](../../mutsukicore/core/handle.py#L75)）—— 这一份代表"调用方持有"。`attach_to(scope)` 不再 +1，只是把 handle 登记到 scope。scope.close 调用 `release()` 释放掉构造时的那一份；如果还有第三方在持有（比如某个 task 里 acquire 了没 release），refcount > 0，handle 仍 alive，就是泄漏。

### TransactionScope：commit / rollback 二选一

[scope.py:170-199](../../mutsukicore/core/scope.py#L170-L199) 扩展了一种"补偿"语义：

```python
class TransactionScope(PluginScope):
    def register_compensation(self, fn: CleanupFn) -> None
    async def commit(self) -> None  # 只调 close()
    async def rollback(self) -> None  # 反向跑补偿，再 close()
```

补偿不在普通 cleanup 列表里。`commit` 不跑补偿；`rollback` 跑补偿（反向，单步失败不阻塞后续），然后再 close 普通 cleanup。

这是 Saga 风格事务的支撑（详见 [TransactionScope 与 Saga](../05-advanced/transaction-scope-saga.md)）。

## 用法示例

最常见的两种登记：

```python
async def on_load(self) -> None:
    # 订阅事件
    unsub = self.bus.subscribe("my-event", self._on_event)
    self.scope.add_subscription(unsub)

    # 起定时任务
    task = asyncio.create_task(self._periodic())
    self.scope.add_timer(task.cancel)

    # 注册服务
    self.services.register(MyService, self)
    self.scope.add_service_registration(
        lambda: self.services.unregister(MyService, self)
    )
```

句柄绑定：

```python
from mutsukicore.contracts.ids import RefId
from mutsukicore.core.handle import RefCountedHandle
from mutsukicore.contracts.refpayload import RefDescriptor

handle = RefCountedHandle(
    target=some_object,
    descriptor=RefDescriptor(
        ref_id=RefId(ctx.id_gen.next("ref")),
        kind="my.thing",
        schema_id_target="my.thing/v1",
        schema_version_target="1.0.0",
    ),
)
handle.attach_to(ctx.scope)
# 此后插件卸载或事务提交时，handle 会被自动 release
```

捕获泄漏：

```python
from mutsukicore.core.scope import HandleLeakError

try:
    await scope.close()
except HandleLeakError as e:
    print(e.error.code)         # ErrorCode("handle.leak")
    print(e.error.evidence)     # 含 leaked_count, cleanup_failures_json...
    print(e.leaked)             # list[RefId]
```

## 常见陷阱

- **scope 不能复用**。`close()` 后再 `add_subscription` 会抛 `RuntimeError("PluginScope(...) 已关闭")`（[scope.py:95-97](../../mutsukicore/core/scope.py#L95-L97)）。
- **`add_timer(task.cancel)` 与 `add_timer(lambda: task.cancel())` 等价但前者更短**——`task.cancel` 本身就是 callable。
- **不要在 cleanup 函数里再注册新 cleanup**。close 的循环已经在跑，新登记会被 `_guard` 拒绝。
- **泄漏时 `cleanup_failures_json` 是字符串**。原始 list[dict] 不能放进 `Error.evidence`（约束只允许标量），所以序列化后塞进去。读的时候要 `json.loads`。
- **句柄一定要先 `attach_to(scope)` 再用**。`acquire` / `borrow` 会先调用 `_check_attached`，未 attach 直接抛 `HandleNotAttachedError`（[handle.py:91-99](../../mutsukicore/core/handle.py#L91-L99)）—— 这是 contracts §11.2 的硬约束。
- **`ResourceKind` 是诊断用，没有按种类的特殊清理**。即便你写错了类别（用 `add_subscription` 登记一个定时器的 cancel），运行时不会区别对待 —— 但泄漏报告里的 `kind` 字段会误导你。
