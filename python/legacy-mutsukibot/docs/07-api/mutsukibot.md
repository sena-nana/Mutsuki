# API · `mutsukibot`

顶层门面，re-export 插件作者最常用的符号。

来源：[mutsukibot/__init__.py](../../mutsukibot/__init__.py)。

## 公开符号

| 符号 | 类型 | 用途 | 详见 |
|---|---|---|---|
| `Agent` | class | 一等运行时实体 | [agent-and-lifecycle](../04-guide/agent-and-lifecycle.md) · [agent.py:55](../../mutsukibot/core/agent.py#L55) |
| `AgentContext` | class | 单次调用上下文 | [agent-context](../04-guide/agent-context.md) · [context.py:32](../../mutsukibot/core/context.py#L32) |
| `Plugin` | class | 插件基类（Generic[Config]） | [plugin-definition](../04-guide/plugin-definition.md) · [plugin.py:416](../../mutsukibot/core/plugin.py#L416) |
| `command` | decorator | 把 async 方法标记为命令 / LLM tool | [command-and-schema](../04-guide/command-and-schema.md) · [plugin.py:99](../../mutsukibot/core/plugin.py#L99) |
| `Capability` | dataclass | 能力声明（name + quantity + policy） | [capability](../04-guide/capability.md) · [capability.py:37](../../mutsukibot/contracts/capability.py#L37) |
| `Caps` | facade | 内置 CapabilityName 常量 | [capability_builtin.py:17](../../mutsukibot/contracts/capability_builtin.py#L17) |
| `Perms` | facade | 内置 PermissionName 常量 | [permission_builtin.py:31](../../mutsukibot/contracts/permission_builtin.py#L31) |
| `Errs` | facade | 内置 ErrorCode 常量 | [error.py:67](../../mutsukibot/contracts/error.py#L67) |
| `LifecyclePhase` | StrEnum | spawn / awake / sleep / stop | [lifecycle.py:6](../../mutsukibot/contracts/lifecycle.py#L6) |
| `Arg` | dataclass | 命令参数约束 + 兜底描述 | [plugin.py:17](../../mutsukibot/contracts/plugin.py#L17) |
| `Inject` | dataclass | 服务注入 sentinel | [plugin.py:37](../../mutsukibot/contracts/plugin.py#L37) |
| `RefArg` | dataclass | 按引用 handle 参数标记 | [plugin.py:47](../../mutsukibot/contracts/plugin.py#L47) |

## 推荐导入风格

```python
from mutsukibot import (
    Agent, AgentContext, Plugin, command,
    Capability, Caps, Perms, Errs, LifecyclePhase,
    Arg, Inject, RefArg,
)
```

需要更多契约（`Message` / `Handle` / `RefDescriptor` / `PermissionRule` …）→ `from mutsukibot.contracts import ...`。需要核心运行时（`PluginScope` / `Bus` / `ServiceContainer` / `Saga` …）→ `from mutsukibot.core.* import ...`。

## 不应直接导入的位置

- `mutsukibot.core.plugin._CommandMarker` 等下划线起头的类
- `mutsukibot.contracts._registered` 私有基类
- `mutsukibot.core.registry._NamedRegistry` 私有泛型

这些在小版本之间可能变化。
