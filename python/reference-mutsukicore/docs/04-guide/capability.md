# Capability

## 这是什么

Capability 是插件**静态声明**的"我能做什么"。它是一个注册式字符串（`CapabilityName`）+ 可选的资源量纲 / 策略元数据，由 `Capability` dataclass 包装。

代码：

- 注册类型：[mutsukicore/contracts/capability.py](../../mutsukicore/contracts/capability.py)
- 内置常量门面：[mutsukicore/contracts/capability_builtin.py](../../mutsukicore/contracts/capability_builtin.py)
- Operation 调用守卫：[mutsukicore/core/capability_guard.py](../../mutsukicore/core/capability_guard.py)

## 解决什么问题

传统框架里"插件能不能调用网络"这种事情靠文档约定 —— 没人在加载时检查。结果生产环境出现 LLM 插件偷偷起本地 socket、调试插件持有 GPU 不释放等情形。

MutsukiCore 的 hard rule #7：**未申报 capability 即调用视为违规**。所有"敏感能力"必须先在 manifest 里列出来；Operation 再用 `requires_capabilities=...` 进一步细粒度声明；dispatcher 在 invoke 前做"required ⊆ declared"检查。

这同时给 Yume 这类 agent 一个清晰的入口去做"我现在能做什么"的内省 —— 它们可以从插件 manifest 直接读出能力图。

## 怎么工作

### CapabilityName：注册式 str 子类

[capability.py:25-34](../../mutsukicore/contracts/capability.py#L25-L34)：

```python
class CapabilityName(RegisteredString):
    _noun: ClassVar[str] = "capability"
    _unknown_error: ClassVar[type[Exception]] = UnknownCapabilityError
    _conflict_error: ClassVar[type[Exception]] = CapabilityConflictError
```

`RegisteredString` 的语义（详见 [registered-strings](../05-advanced/registered-strings.md)）：

- 构造时强制要求已注册：`CapabilityName("read_message")` 仅对此前调用过 `register` 的名字有效，否则抛 `UnknownCapabilityError`
- 同 owner 重注册幂等
- 跨 owner 注册同名抛 `CapabilityConflictError`

这让框架核心可以预先注册一组名字（[capability_builtin.py:31-44](../../mutsukicore/contracts/capability_builtin.py#L31-L44)），第三方插件扩展自己的命名空间（如 `yume.vram`）—— 而 `CapabilityName("typo_here")` 这种打字错误会在构造时直接拒掉。

### 内置 Caps 门面

[capability_builtin.py:17-28](../../mutsukicore/contracts/capability_builtin.py#L17-L28)：

```python
class Caps:
    READ_MESSAGE: ClassVar[CapabilityName]
    SEND_MESSAGE: ClassVar[CapabilityName]
    CALL_LLM: ClassVar[CapabilityName]
    PERSIST: ClassVar[CapabilityName]
    NETWORK_EGRESS: ClassVar[CapabilityName]
    SPAWN_AGENT: ClassVar[CapabilityName]
    HOLD_REF: ClassVar[CapabilityName]
    BORROW_REF: ClassVar[CapabilityName]
    PRODUCE_REF_STREAM: ClassVar[CapabilityName]
```

下面紧跟一行 `CapabilityName.bootstrap_facade(...)` 把这些类属性填好。pyright 能完整推断 `Caps.READ_MESSAGE` 的类型是 `CapabilityName`。

### Capability 数据类

[capability.py:37-46](../../mutsukicore/contracts/capability.py#L37-L46)：

```python
class Capability(Contract):
    name: CapabilityName
    quantity: dict[str, int | str] | None = None
    policy: dict[str, str] | None = None
```

`quantity` 用来表达"能用多少"——比如 `{"tokens_per_min": 100000}`、`{"bytes": 1_000_000_000}`。`policy` 表达额外约束 —— 比如 `{"allowed_hosts": "api.openai.com,api.anthropic.com"}`。

当前 dispatcher 调用路径只校验 `name`；`quantity` / `policy` 当作 manifest 元数据让上层（配额系统、审计）使用。

### 守卫：required ⊆ declared

[capability_guard.py:20-36](../../mutsukicore/core/capability_guard.py#L20-L36)：

```python
def check_capabilities(
    *,
    plugin_id: str,
    declared: tuple[CapabilityName, ...],
    required: tuple[CapabilityName, ...],
    route: str,
) -> None:
    declared_set = set(declared)
    missing = tuple(c for c in required if c not in declared_set)
    if missing:
        err = Error(
            code=Errs.CAPABILITY_NOT_DECLARED,
            source=plugin_id,
            route=route,
            evidence={"missing": ",".join(missing)},
        )
        raise CapabilityNotDeclaredError(missing, err)
```

dispatcher 在 Operation invoke 路径调用：

- `declared` = 插件类的 `capabilities` 列表的 name 集合
- `required` = 命令 `@command(requires_capabilities=...)` 列表

任何 `required` 不在 `declared` 里的 capability 都会让调用拒绝执行，抛出携带 `Error(code=Errs.CAPABILITY_NOT_DECLARED)` 的结构化 wrapper。

## 用法示例

声明 capability 给整个插件：

```python
class WebSearchPlugin(Plugin[Cfg]):
    capabilities = [
        Capability(name=Caps.NETWORK_EGRESS, policy={"allowed_hosts": "duckduckgo.com"}),
        Capability(name=Caps.PERSIST, quantity={"bytes": 10_000_000}),
    ]
```

为单个命令声明额外要求（必须是上面已声明的子集）：

```python
@command(requires_capabilities=(Caps.NETWORK_EGRESS,))
async def search(self, query: str) -> str:
    ...
```

注册自有 capability：

```python
from mutsukicore.contracts.capability import CapabilityName

# 在你的插件包初始化处一次性注册
YumeCaps_VRAM = CapabilityName.register(
    "yume.vram",
    declared_by="yume.runtime",
)

class MyPlugin(Plugin[Cfg]):
    capabilities = [Capability(name=YumeCaps_VRAM, quantity={"mib": 8192})]
```

按 [capability_builtin.py](../../mutsukicore/contracts/capability_builtin.py) 的模式建一个门面类，让 IDE 能补全：

```python
class YumeCaps:
    VRAM: ClassVar[CapabilityName]
    KV_CACHE: ClassVar[CapabilityName]


CapabilityName.bootstrap_facade(
    YumeCaps,
    {
        "VRAM": "yume.vram",
        "KV_CACHE": "yume.kv_cache",
    },
    declared_by="yume.runtime",
)
```

## 常见陷阱

- **构造未注册的 `CapabilityName` 会立即抛错**。导入时序很重要：如果你在某模块顶层 `Caps.READ_MESSAGE`，必须保证 `mutsukicore.contracts.capability_builtin` 已经被导入过（通常 `from mutsukicore import Caps` 或 `from mutsukicore.contracts import Caps` 就够了）。
- **`requires_capabilities` 不会自动收录到插件 declared**。Operation 声明 `requires_capabilities=(Caps.PERSIST,)`，但插件 `capabilities` 没列 `PERSIST` —— dispatcher 拒绝执行。
- **不同 owner 注册同名 capability 会冲突**。两家插件都想叫 "memory"，第二家在 `register` 时抛 `CapabilityConflictError`。约定带前缀（`yume.memory`、`mind_sim.memory`）。
- **manifest 里的 quantity 仍只是声明元数据**。真正的容量计数走 `ResourceHost.declare_capacity()` / `acquire()`；超额时会触发 `Errs.CAPABILITY_EXHAUSTED`。
- **守卫只在 Operation 调用时跑**。在 `on_load` / `on_unload` 里直接做敏感操作不会被拦 —— 这是当前实现限制。把敏感操作放 Operation 里走 dispatcher 路径才能享受守卫。
