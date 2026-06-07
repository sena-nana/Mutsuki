# 命令与 Schema 生成

## 这是什么

`@command` 装饰的方法**同时**是两件东西：

1. 给人类（或 adapter）触发的"命令"（`text echo hello` → 路由到此方法）
2. 给 LLM 调用的"工具"（`{"name": "echo", "parameters": {...}}`）

它们共用一份函数签名、一份 docstring、一份 JSON Schema。这是 [AGENTS.md](../../AGENTS.md) 的 hard rule #5「指令即工具」。

代码：[`_build_command_spec`](../../mutsukibot/core/plugin.py#L170-L274) 在 [mutsukibot/core/plugin.py](../../mutsukibot/core/plugin.py)。

## 解决什么问题

传统 bot 框架里，「人类命令」与「LLM tool 调用」往往要写两份：一份是 `argparse`-style 的命令解析器，一份是手工维护的 OpenAI tools schema。它们容易漂移：你加了个参数，命令路由更新了，LLM 那边的 schema 还停在旧版。

MutsukiBot 的做法是：**只让作者写一种东西**——一个带类型注解和 Google-style docstring 的 async 函数——schema 由元类自动从签名 + docstring + `Annotated[..., Arg(...)]` 合成。

## 怎么工作

### 信息源融合

`_build_command_spec` 接收三种信息，按优先级合并：

| 来源 | 提供 | 优先级 |
|---|---|---|
| `@command(name=, desc=)` 关键字 | 显式覆盖名字与描述 | 最高 |
| docstring（Google 风格） | 描述 + 参数描述 | 次之 |
| `Annotated[T, Arg(...)]` 元数据 | 约束（ge/le/min_length/regex/...）+ 兜底描述 | 始终 |

具体合并代码在 [plugin.py:178-249](../../mutsukibot/core/plugin.py#L178-L249)。

### 描述

[plugin.py:182-186](../../mutsukibot/core/plugin.py#L182-L186)：

```python
description = (
    marker.explicit_desc
    or (parsed.short_description if parsed and parsed.short_description else "")
    or fn_name
)
```

——`@command(desc=...)` > docstring 首段 > 函数名兜底。

### 参数描述

[plugin.py:188-191](../../mutsukibot/core/plugin.py#L188-L191)：

```python
param_descs: dict[str, str] = {}
if parsed is not None:
    for p in parsed.params:
        if p.description:
            param_descs[p.arg_name] = p.description.strip()
```

`docstring_parser` 解析 Google-style `Args:` 段。每个 `key: description` 落到 `param_descs[key]`。

如果某参数 docstring 里没写描述，但 `Annotated[..., Arg(desc="...")]` 里写了，会作为 fallback 填进去（[plugin.py:243-245](../../mutsukibot/core/plugin.py#L243-L245)）。设计意图见 [contracts.plugin.Arg 注释](../../mutsukibot/contracts/plugin.py#L17-L24) —— **描述应该来自 docstring，`Arg` 主要承载约束**。

### 类型 → JSON Schema 类型

[plugin.py:145-167](../../mutsukibot/core/plugin.py#L145-L167) 的 `_json_type_for`：

| Python | JSON |
|---|---|
| `str` | `string` |
| `int` | `integer` |
| `float` | `number` |
| `bool` | `boolean` |
| 其他（含自定义类） | 退化为 `string` |

`Annotated[T, ...]` 会先剥到内层类型再判断。

### 约束

`Annotated[T, Arg(...)]` 里的 `Arg` 字段直接映射到 JSON Schema 的标准 keyword：

| `Arg` 字段 | JSON Schema |
|---|---|
| `ge` | `minimum` |
| `le` | `maximum` |
| `gt` | `exclusiveMinimum` |
| `lt` | `exclusiveMaximum` |
| `min_length` | `minLength` |
| `max_length` | `maxLength` |
| `regex` | `pattern` |
| `choices` | `enum`（列表化） |

完整代码在 [plugin.py:225-243](../../mutsukibot/core/plugin.py#L225-L243)。

### 哪些参数会被略过

[plugin.py:209-222](../../mutsukibot/core/plugin.py#L209-L222) 决定：以下两类参数**不**进 schema —— 它们由框架注入，不该让外部调用方看到：

1. 类型是 `AgentContext`（含子类）
2. 默认值是 `Inject()` sentinel

详见 [依赖注入](dependency-injection.md)。

### required 字段

[plugin.py:251-252](../../mutsukibot/core/plugin.py#L251-L252)：

```python
if sig_param.default is inspect.Parameter.empty:
    required.append(pname)
```

没有默认值的参数即视为 required。

### 返回值

如果函数声明了返回类型，`return_schema` 会按同样规则生成（[plugin.py:260-262](../../mutsukibot/core/plugin.py#L260-L262)）。当前 runtime 不做返回值强制校验 —— scheduler 直接 `str(result)` 后塞进出站消息。

### CommandSpec 的形态

`CommandSpec`（[contracts/plugin.py:90-104](../../mutsukibot/contracts/plugin.py#L90-L104)）是 `Contract`（即 `msgspec.Struct`）：

```python
class CommandSpec(Contract):
    name: str
    description: str
    plugin_id: str
    func_qualname: str
    parameters_schema: dict[str, Any] = {}
    return_schema: dict[str, Any] = {}
    perms_rule_id: str | None = None
    requires_capabilities: tuple[CapabilityName, ...] = ()
    is_tool: bool = True
```

LLM tool 桥接插件可以遍历 `agent.plugins[*].plugin.__commands__` 把所有 `is_tool=True` 的 spec 转成自己 provider 的 tool schema。

## 用法示例

```python
@command(perms=Perms.PUBLIC)
async def search(
    self,
    query: Annotated[str, Arg(min_length=1, max_length=100)],
    top_k: Annotated[int, Arg(ge=1, le=20)] = 5,
    domain: Annotated[str, Arg(choices=("web", "code", "doc"))] = "web",
) -> str:
    """模糊检索。

    Args:
        query: 检索词。
        top_k: 返回前 k 条。
        domain: 检索域。
    """
    ...
```

合成出的 `parameters_schema`：

```json
{
  "type": "object",
  "properties": {
    "query": {
      "type": "string",
      "minLength": 1,
      "maxLength": 100,
      "description": "检索词。"
    },
    "top_k": {
      "type": "integer",
      "minimum": 1,
      "maximum": 20,
      "description": "返回前 k 条。"
    },
    "domain": {
      "type": "string",
      "enum": ["web", "code", "doc"],
      "description": "检索域。"
    }
  },
  "required": ["query"]
}
```

## 常见陷阱

- **不要在描述里依赖 reST 段落**。当前用 Google 风格解析；reST 的 `:param x:` 不会被识别。
- **`Arg(desc=...)` 是 fallback，不是首选**。优先写 docstring；`Arg` 用来表达约束。
- **复杂类型暂时退化为 string**。v0.1 schema 合成不递归处理嵌套结构（list / dict / 自定义 Struct）。如果你需要传结构化参数，目前的实践是定义一个 service 接口而不是命令参数。
- **`func_qualname` 用于错误归因**，比如 trace 里出现 `plugin.mutsukibot-echo.echo`，方便回溯到源码 —— 不要为了 LLM 友好就给函数起特殊名字。
- **schema 与人类命令解析共用同一份**。Scheduler 用 `parameters_schema` 的 properties 顺序把 shell-style 位置参数对齐到 kwargs（[scheduler.py:152-155](../../mutsukibot/runtime/scheduler.py#L152-L155)），并按 `type` 做粗粒度强转（int / float / bool）。所以**参数声明顺序 = 命令位置参数顺序**。
