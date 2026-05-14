# 插件定义与 PluginMeta

## 这是什么

MutsukiBot 的插件是**类**，继承 `Plugin[Config]`，再用 `@command` 装饰几个 async 方法。表面 API 极其紧凑；幕后由元类 `PluginMeta` 在 class 语句求值时做完所有校验、收集与注册。

代码：

- 元类 + 基类：[mutsukibot/core/plugin.py](../../mutsukibot/core/plugin.py)
- 装饰器侧的 sentinel：[mutsukibot/contracts/plugin.py](../../mutsukibot/contracts/plugin.py)
- 参考实现：[mutsukibot/plugins/echo/__init__.py](../../mutsukibot/plugins/echo/__init__.py)

## 解决什么问题

如果用普通 `__init_subclass__` 或函数注册，插件作者很容易：

- 漏写 `id` / `version` / `capabilities` —— 直到运行时才报错
- 错把 `Config` 写成 dataclass 而非 `msgspec.Struct` —— 装载时反序列化失败
- 装饰器写错（同步方法 / 漏写参数）—— 运行时抛 TypeError

PluginMeta 把这些校验前移到**类定义那一行**，错误指向定义点本身。

## 怎么工作

### 用户侧的最小形态

```python
from typing import Annotated, ClassVar
import msgspec
from mutsukibot import Capability, Caps, Perms, Plugin, command
from mutsukibot.contracts import Arg


class _MyConfig(msgspec.Struct, kw_only=True):
    prefix: str = "hi: "


class MyPlugin(Plugin[_MyConfig]):
    id: ClassVar[str] = "my-plugin"
    version: ClassVar[str] = "0.1.0"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _MyConfig

    @command(perms=Perms.PUBLIC)
    async def hello(self, name: Annotated[str, Arg(min_length=1)]) -> str:
        """问好。

        Args:
            name: 对方名字。
        """
        return f"{self.config.prefix}{name}"
```

这就是全部。

### PluginMeta 在 `class` 语句求值时做的 7 件事

完整流程在 [plugin.py:285-403](../../mutsukibot/core/plugin.py#L285-L403)：

1. **校验必需 ClassVar**。`id` / `version` / `capabilities` 任缺一项都立刻抛 `PluginDefinitionError`，并在 `error.evidence` 里给出 `missing` 与 `plugin_class`（[plugin.py:303-314](../../mutsukibot/core/plugin.py#L303-L314)）。
2. **校验嵌套 `Config`**。`Config` 必须存在且是 `msgspec.Struct` 子类，否则同样抛错（[plugin.py:323-336](../../mutsukibot/core/plugin.py#L323-L336)）。
3. **扫描 `@command`-标记方法**。`namespace` 里每个挂了 `_CommandMarker` 的属性都被收集进 `markers: dict[attr_name, _CommandMarker]`。
4. **解析签名 → `Dependent`**，立即缓存到 `marker.dependent`（[plugin.py:347](../../mutsukibot/core/plugin.py#L347)）。如果签名里有任何参数无法被四类 Param 认领，就在这里抛 `PluginDefinitionError` —— 调度器永远不会跑反射。
5. **构造 `CommandSpec`**：合并 docstring 与 `Annotated[..., Arg(...)]` 生成 JSON Schema（详见 [命令与 Schema](command-and-schema.md)），缓存到 `marker.spec`（[plugin.py:363-370](../../mutsukibot/core/plugin.py#L363-L370)）。
6. **构造静态 `PluginManifest`**，挂到 `cls.__manifest__`（[plugin.py:379-390](../../mutsukibot/core/plugin.py#L379-L390)）。同时把所有命令规约写到 `cls.__commands__`，把 marker 字典写到 `cls.__command_markers__`（agent 加载时直接读这两个，不再 inspect）。
7. **登记到 `PluginRegistry`** 并写入 `__source_location__`（含 `file:line`）方便调试（[plugin.py:393-401](../../mutsukibot/core/plugin.py#L393-L401)）。

为什么要用真元类，而不是 `__init_subclass__`：[plugin.py:12-19](../../mutsukibot/core/plugin.py#L12-L19) 给出的解释 —— manifest 字段以 ClassVar 形式声明（pyright 友好），不是 class 语句的关键字参数；元类直接从 `cls.__dict__` 读取，校验在 class 语句求值时立即跑，错误指向定义点本身。

### Plugin 基类的 `__init__`

Loader 实例化插件时（[loader.py:112-118](../../mutsukibot/core/loader.py#L112-L118)）会以 5 个 keyword-only 参数注入：

```python
def __init__(
    self,
    *,
    agent: "Agent",
    config: C,
    scope: "PluginScope",
    services: "ServiceContainer",
    bus: "Bus",
) -> None:
    self.agent = agent
    self.config: C = config
    self.scope = scope
    self.services = services
    self.bus = bus
```

子类**通常不重写** `__init__`。要做初始化（订阅事件、起定时器、注册服务）就重写 `on_load()`：

```python
async def on_load(self) -> None:
    unsub = self.bus.subscribe("my-event", self._on_my_event)
    self.scope.add_subscription(unsub)
```

`on_unload()` 用于显式清理。但即便不写，scope 仍会被 loader 关闭（自动回收订阅 / 句柄等），所以 `on_unload` 主要用于"清理 scope 之外的东西"（极少见）。

### 装饰器：`@command` 只是打个标记

[plugin.py:99-137](../../mutsukibot/core/plugin.py#L99-L137)：

```python
def command(
    *,
    name: str | None = None,
    desc: str | None = None,
    perms: PermissionRule | PermissionName | None = None,
    requires_capabilities: tuple[CapabilityName, ...] = (),
    is_tool: bool = True,
) -> ...
```

- `name` —— 显式覆盖命令名（默认是函数名）
- `desc` —— 显式覆盖描述（默认从 docstring 首段取）
- `perms` —— 一个 `PermissionRule` 或 `PermissionName`（默认 `Perms.PUBLIC`）
- `requires_capabilities` —— 此命令额外需要的 capability，必须是插件已声明集合的子集（详见 [Capability](capability.md)）
- `is_tool` —— 是否同时作为 LLM tool（默认 `True`，即 hard rule #5「指令即工具」）

装饰器立刻校验 "必须是 `async def`"（[plugin.py:123-126](../../mutsukibot/core/plugin.py#L123-L126)），然后只是把 `_CommandMarker` 挂到函数对象上。真正的 `CommandSpec` 与 `Dependent` 由 PluginMeta 在所属类体求值完毕后构建 —— 那时 docstring、签名、所属类都已就位。

## 用法示例

完整对照 [mutsukibot/plugins/echo/__init__.py](../../mutsukibot/plugins/echo/__init__.py)：

```python
class _EchoConfig(msgspec.Struct, kw_only=True):
    prefix: str = "echo: "


class EchoPlugin(Plugin[_EchoConfig]):
    """回显输入文本。展示标准插件形态。"""

    id: ClassVar[str] = "mutsukibot-echo"
    version: ClassVar[str] = "0.1.0"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _EchoConfig

    @command(perms=Perms.PUBLIC)
    async def echo(
        self,
        text: str,
        count: Annotated[int, Arg(ge=1, le=10)] = 1,
    ) -> str:
        """回显输入文本。

        Args:
            text: 要回显的文本。
            count: 重复次数（1–10）。
        """
        return (self.config.prefix + text + "\n") * count
```

调用方拿到的：

- `EchoPlugin.id == "mutsukibot-echo"`
- `EchoPlugin.__manifest__` 是一个完整 `PluginManifest`
- `EchoPlugin.__commands__` 含一个 `CommandSpec(name="echo", parameters_schema={...})`
- `EchoPlugin.__source_location__` 形如 `".../mutsukibot/plugins/echo/__init__.py:22"`
- `PluginRegistry["mutsukibot-echo"]` 返回 `EchoPlugin` 类本身

## 常见陷阱

- **`@command` 必须装饰 `async def`**。装饰同步方法会立刻 `TypeError`。
- **`Config = SomeStruct` 不可省**。即便配置全是默认值，也要写一个空的 `class Config(msgspec.Struct, kw_only=True): pass`。
- **`id` 必须 kebab-case 且全局唯一**。冲突时 `PluginRegistry` 会拒绝。如果你需要在测试里反复装载同一个插件，让 loader 在 `unload_from` 里走"卸载实例 → 重新登记类"的流程（[loader.py:131-133](../../mutsukibot/core/loader.py#L131-L133)），不要手动 register。
- **`requires_plugins` 写错插件 id 会被拓扑器忽略**。`_toposort` 只对已知节点排序，外部依赖被过滤掉（[loader.py:50](../../mutsukibot/core/loader.py#L50)）。这避免"依赖未安装就启动失败"，但也意味着拼写错误不会立即报错 —— 命令调用时才会发现服务/插件不存在。
- **不要在 `__init__` 里干活**。把订阅、定时器、服务注册都放在 `on_load`，因为 `__init__` 阶段 `self.scope` 已经存在但调用方还没拿到实例引用 —— 错误更难追踪。
