# 依赖注入

## 这是什么

`Dependent[R]` 把一个 `async def fn(...) -> R` 的签名，在 class 定义时一次性解析成 `Param` 列表；调用时按列表求值，把每个参数填好后 `await fn(**kwargs)`。

代码：[mutsukibot/core/dependency.py](../../mutsukibot/core/dependency.py)。

## 解决什么问题

NoneBot 的 `Depends(...)` 系统证明了「按签名声明依赖」的工程价值：作者只关心"我需要什么"，框架负责"从哪里拿"。但 NoneBot 允许按参数名 fallback —— 一个名为 `bot` 的参数会自动注入当前 Bot 实例。MutsukiBot 不这么做：参数必须**显式被某种 Param 认领**，否则在 parse 阶段就拒绝。原因是：

1. 按名 fallback 让重命名变 breaking change
2. 让"漏写注解"从 bug 变成"运气好"
3. 多人协作时增加心智负担

MutsukiBot 的妥协：四类 Param + 严格 claim 流程，保留 NoneBot 心智，但加上类型门槛。

## 怎么工作

### 四类 Param

| Param | 认领条件 | 注入什么 | 代码 |
|---|---|---|---|
| `CtxParam` | 类型是 `AgentContext`（含子类） | 当前 ctx | [dependency.py:71-93](../../mutsukibot/core/dependency.py#L71-L93) |
| `RefParam` | `Annotated[..., RefArg(kind="...")]` | 从 `extras` 取同名 handle | [dependency.py:149-166](../../mutsukibot/core/dependency.py#L149-L166) |
| `ServiceParam` | 默认值是 `Inject()` sentinel | `ctx.services.resolve(类型, name=inject.name)` | [dependency.py:128-146](../../mutsukibot/core/dependency.py#L128-L146) |
| `ArgParam` | 以上都不认领，且不是 `Inject` 默认值 | `extras[name]`；缺则用默认值；再缺抛 `KeyError` | [dependency.py:96-125](../../mutsukibot/core/dependency.py#L96-L125) |

claim 顺序由 `_DEFAULT_PARAMS` 决定（[dependency.py:169](../../mutsukibot/core/dependency.py#L169)）：`CtxParam → RefParam → ServiceParam → ArgParam`。第一个返回非 None 的就赢，后续不再尝试。

### parse：发生在类定义时

[dependency.py:179-235](../../mutsukibot/core/dependency.py#L179-L235) 的 `Dependent.parse(call)`：

1. `inspect.signature(call)` 取签名
2. `get_type_hints(call, include_extras=True)` 解析注解（保留 `Annotated` 元数据）
3. 跳过 `self`（默认行为）；遇到 `*args` / `**kwargs` 直接拒绝
4. 遇到没注解的参数直接拒绝
5. 对每个参数构造 `ParameterInfo`，依次问每个 `Param.claim(info)`，第一个成功的进 `resolved` 列表
6. 没有任何 Param 认领 → 抛 `UnresolvedParameterError`

`PluginMeta` 在收集命令时调用 `Dependent.parse` 并把结果缓存到 `marker.dependent`（[plugin.py:347](../../mutsukibot/core/plugin.py#L347)）。这样 scheduler 调度时**完全不需要做反射**——所有 Param 的 claim 与签名解析都在类定义时跑过。

### solve：发生在每次调用时

[dependency.py:237-248](../../mutsukibot/core/dependency.py#L237-L248)：

```python
async def solve(
    self,
    ctx: "AgentContext",
    bound_self: object | None = None,
    **extras: Any,
) -> R:
    kwargs: dict[str, Any] = {}
    for param in self.params:
        kwargs[param.info.name] = await param.solve(ctx, **extras)
    if bound_self is not None:
        return await self.call(bound_self, **kwargs)
    return await self.call(**kwargs)
```

每个 Param 拿到完整 ctx + extras，自己挑需要的。`bound_self` 用于绑定方法调用：scheduler 传 `bound_self=plugin` 让方法看到真正的 `self`（[scheduler.py:167](../../mutsukibot/runtime/scheduler.py#L167)）。

### extras 是什么

调度器从 shell-style 命令行解析出的位置参数，按 `parameters_schema` 的 properties 顺序对齐成 dict，传给 solve（[scheduler.py:152-155](../../mutsukibot/runtime/scheduler.py#L152-L155)）：

```python
param_names = list(spec.parameters_schema.get("properties", {}))
extras: dict[str, object] = {}
for name, value in zip(param_names, positional, strict=False):
    extras[name] = _coerce(value, spec.parameters_schema["properties"][name])
```

如果是 LLM tool 调用，桥接插件应当把 `{"text": "...", "count": 3}` 这样的 dict 直接作为 extras 传进 `dependent.solve(ctx, bound_self=plugin, **dict)` —— 同一份签名两条路径共用。

## 用法示例

四种参数全凑齐的命令：

```python
from typing import Annotated, Any
from mutsukibot import AgentContext, Inject, Plugin, command
from mutsukibot.contracts import Handle, RefArg

class MyPlugin(Plugin[Config]):
    ...

    @command()
    async def process(
        self,
        ctx: AgentContext,                                # CtxParam
        text: str,                                        # ArgParam（来自命令位置参数）
        repeats: int = 1,                                 # ArgParam，有默认值
        client: HttpClient = Inject(),                    # ServiceParam，按类型解析
        latent: Annotated[                                # RefParam，按 kind 拿 handle
            Handle[Any], RefArg(kind="yume.latent")
        ] = ...,
    ) -> str:
        ...
```

调度时 scheduler 给 `extras = {"text": ..., "repeats": ...}`，solve 后变成：

| 参数 | Param | 来源 |
|---|---|---|
| `ctx` | CtxParam | `ctx` 本身 |
| `text` | ArgParam | `extras["text"]` |
| `repeats` | ArgParam | `extras.get("repeats", 1)` |
| `client` | ServiceParam | `ctx.services.resolve(HttpClient)` |
| `latent` | RefParam | `extras["latent"]`（必须由调用方提供） |

## 常见陷阱

- **没注解的参数直接报错**。`async def foo(self, x):` 这种写法 PluginMeta 会把它拒掉，错误指向类定义所在行。
- **`*args` / `**kwargs` 不支持**。如果你需要可变参数，定义一个 `list` / `dict` 参数，让调用方显式打包。
- **`Inject()` 是默认值，不是注解**。写法是 `svc: HttpClient = Inject()`，不是 `svc: Annotated[HttpClient, Inject()]`。后者会被 ArgParam 认领，因为 ServiceParam 只看 default 值。
- **`RefArg(kind="...")` 的 kind 是文档性的**。当前 v0.1 RefParam 不校验 handle 的 descriptor.kind 是否匹配 —— 只是按参数名从 extras 取。校验由调用方负责，或在你自己的桥接插件里加。
- **claim 顺序固定**。如果你既写 `Annotated[..., RefArg(...)]` 又给默认值 `Inject()`，RefParam 先赢（它在 `_DEFAULT_PARAMS` 排第二，ServiceParam 第三）。
- **同一个签名在 PluginMeta 解析后就冻结了**。在 `__init__` / `on_load` 里动态修改方法签名不会被框架感知 —— scheduler 只看缓存好的 `marker.dependent`。
