# Permission

## 这是什么

Permission 是命令的**运行时准入谓词**——「当下这个调用者，在这个上下文里，能不能调用我」。它和 [Capability](capability.md) 正交：capability 表达「插件能做什么」（静态），permission 表达「现在允不允许」（动态）。

代码：

- 规则 AST + 注册名：[nanobot/contracts/permission.py](../../nanobot/contracts/permission.py)
- 内置常量门面：[nanobot/contracts/permission_builtin.py](../../nanobot/contracts/permission_builtin.py)

## 解决什么问题

传统框架的 "permission decorator" 通常只支持简单字符串名（`@admin_only`）或硬编码角色检查。一旦组合需求出现 —— "owner 或 在 ops 频道" —— 就要套两层装饰器或者写一个新名字。NanoBot 借鉴 NoneBot 的 `Rule` / `Permission` 思路，把它合并成一种类型 `PermissionRule`，并支持 `&` / `|` 组合，AST 求值时严格保留布尔语义。

## 怎么工作

### PermissionRule：AST + check

[permission.py:40-84](../../nanobot/contracts/permission.py#L40-L84)：

```python
class PermissionRule:
    async def check(self, ctx: "AgentContext") -> bool: ...
    @classmethod
    def from_checker(cls, fn: CheckerFn) -> "PermissionRule": return _Leaf(fn)
    @classmethod
    def always(cls) -> "PermissionRule": ...
    @classmethod
    def never(cls) -> "PermissionRule": ...
    def __and__(self, other) -> "PermissionRule": ...  # 平展同类节点
    def __or__(self, other) -> "PermissionRule": ...   # 平展同类节点
```

三个节点类型：

| 节点 | check 行为 |
|---|---|
| `_Leaf(checker)` | 直接 await checker(ctx) |
| `_And(parts)` | 短路 AND：任一返回 False 即停 |
| `_Or(parts)` | 短路 OR：任一返回 True 即停 |

`__and__` / `__or__` 在组合时把同类节点平展（[permission.py:72-84](../../nanobot/contracts/permission.py#L72-L84)），AST 保持浅；但**不**进一步合并 AND / OR —— `(a | b) & (c | d)` 严格按 (a OR b) AND (c OR d) 求值，而不是退化成四项 OR。

### PermissionName：可注册的命名规则

[permission.py:117-157](../../nanobot/contracts/permission.py#L117-L157)：

```python
class PermissionName(RegisteredString):
    _checker: ClassVar[dict[str, CheckerFn]] = {}

    @classmethod
    def register(cls, name: str, *, declared_by: str, checker: CheckerFn):
        instance = cls._intern(name, declared_by=declared_by)
        cls._checker.setdefault(name, checker)
        return instance

    def to_rule(self) -> PermissionRule:
        return PermissionRule.from_checker(self._checker[self])
```

每个名字关联一个 checker。`PermissionName & PermissionRule` 与 `PermissionName & PermissionName` 都自动调用 `to_rule` 后再组合（[permission.py:145-157](../../nanobot/contracts/permission.py#L145-L157)）。

### 内置 Perms 门面

[permission_builtin.py](../../nanobot/contracts/permission_builtin.py) 注册两个：

```python
class Perms:
    PUBLIC: ClassVar[PermissionName]      # 永远 True
    AGENT_OWNER: ClassVar[PermissionName] # 仅当 ctx.message.source.user_id == ctx.agent_owner
```

注意 `_agent_owner` 的实现里，**`ctx.message is None` 时返回 True**——这意味着生命周期钩子里调用是允许的。命令路径里 `message` 一定有值。

### 调度器侧 await check

[scheduler.py:139-149](../../nanobot/runtime/scheduler.py#L139-L149)：

```python
if not await marker.perms.check(ctx):
    await self._emit_error(
        msg,
        Error(
            code=Errs.PERMISSION_DENIED,
            source=plugin.id,
            route=f"command.{spec.name}",
            evidence={"perms_rule": spec.perms_rule_id or ""},
        ),
    )
    return
```

permission 检查发生在 capability 检查之后、参数解析之前。失败立即返回 `Errs.PERMISSION_DENIED`。

### `@command(perms=...)` 接受三种值

[plugin.py:114-120](../../nanobot/core/plugin.py#L114-L120)：

```python
if perms is None:
    rule = Perms.PUBLIC.to_rule()
elif isinstance(perms, PermissionName):
    rule = perms.to_rule()
else:
    rule = perms  # 已经是 PermissionRule
```

写法因此可以是：

- `@command()` —— 默认 PUBLIC
- `@command(perms=Perms.AGENT_OWNER)` —— 单个命名权限
- `@command(perms=Perms.AGENT_OWNER & MyPerms.IN_OPS_CHANNEL)` —— 组合规则
- `@command(perms=PermissionRule.from_checker(my_async_fn))` —— 临时谓词

## 用法示例

注册自有 PermissionName：

```python
from nanobot import AgentContext
from nanobot.contracts.permission import PermissionName

async def _is_in_ops_channel(ctx: AgentContext) -> bool:
    if ctx.message is None:
        return False
    return ctx.message.source.channel_id == "ops"

IN_OPS = PermissionName.register(
    "ops.channel",
    declared_by="my-plugin",
    checker=_is_in_ops_channel,
)

class MyPerms:
    IN_OPS_CHANNEL = IN_OPS  # 提供门面属性
```

组合规则：

```python
@command(perms=Perms.AGENT_OWNER & MyPerms.IN_OPS_CHANNEL)
async def restart(self, ctx: AgentContext) -> str:
    ...
```

或者 OR 组合：

```python
@command(perms=Perms.AGENT_OWNER | MyPerms.IN_OPS_CHANNEL)
async def status(self, ctx: AgentContext) -> str:
    ...
```

完全自定义匿名规则：

```python
async def _heavy_check(ctx: AgentContext) -> bool:
    user = ctx.message.source.user_id if ctx.message else None
    return user in await db.allowlist()

rule = PermissionRule.from_checker(_heavy_check)

@command(perms=rule)
async def admin(self, ctx: AgentContext) -> str:
    ...
```

## 常见陷阱

- **checker 是 async**。即便实现里只是返回常量，签名也必须是 `async def`。
- **`PermissionName.register` 是幂等 + owner 排他**。同 owner 多次注册同名只用第一次的 checker（`setdefault` 行为，[permission.py:138-139](../../nanobot/contracts/permission.py#L138-L139)）；不同 owner 抛 `PermissionConflictError`。
- **`perms_rule_id` 只是诊断标签**。它不参与求值，只在 `Errs.PERMISSION_DENIED` 的 evidence 里出现，方便定位是哪个命令拒了。它由 PluginMeta 自动设成 `f"{plugin_id}.{attr}"`（[plugin.py:367](../../nanobot/core/plugin.py#L367)）。
- **拒绝后命令完全不跑**——参数也不会被解析。这避免了"先拼参数发现没权限就丢掉"的浪费。
- **AND / OR 不会自动合并**：`(a | b) & (c | d)` 不会被简化成四项 OR。如果你想要那种语义，自己写 `a | b | c | d`。
- **匿名 `from_checker` 无法被 `PermissionName` 反查**。它的 `perms_rule_id` 在错误诊断里会是 `f"{plugin_id}.{attr}"` 形式，不指向 checker 函数本身。要可观测就用 `PermissionName.register`。
