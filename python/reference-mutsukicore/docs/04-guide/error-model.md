# Error 模型

## 这是什么

MutsukiCore 的错误是**结构化数据对象**，不是字符串异常。`Error` 含稳定 code、source、route、可选 lost_capability、recovery 提示、cause 链、evidence 字典。错误码本身（`ErrorCode`）是注册式字符串。

代码：[mutsukicore/contracts/error.py](../../mutsukicore/contracts/error.py)。

## 解决什么问题

[AGENTS.md hard rule #8](../../AGENTS.md)：**结构化错误，不允许吞异常返默认值**。理由：

1. 字符串错误无法路由 —— 上层想根据错误类型决定 retry / fallback / abort，必须正则匹配字符串
2. 字符串错误无法测试 —— "包含 `not found`" 这种断言会随措辞变化而坏
3. 隐式 fallback 把"丢能力"伪装成"成功" —— Yume 的 latent 链路出错降级到 text 时如果不显式记录就找不到

`Error` 把这些都做成一等数据。

## 怎么工作

### Error 数据形态

[error.py:41-61](../../mutsukicore/contracts/error.py#L41-L61)：

```python
class Error(Contract):
    code: ErrorCode
    source: str
    route: str
    lost_capability: CapabilityName | None = None
    recovery: RecoveryAction | None = None
    cause: "Error | None" = None
    evidence: dict[str, str | int | float | bool] = {}

    def chain(self) -> list[Self]:
        result: list[Self] = []
        cur: Self | None = self
        while cur is not None:
            result.append(cur)
            cur = cur.cause
        return result
```

- `code` —— 稳定字符串，可路由、可断言、可指标化
- `source` —— 谁触发的（插件 id / 模块名）
- `route` —— 调用路径（如 `command.echo` / `scope.close` / `plugin.dag`）
- `lost_capability` —— 因错误丢失的能力（容许显式降级时可填）
- `recovery` —— `RecoveryAction.RETRY / FALLBACK / ESCALATE / ABORT`
- `cause` —— 链式 wrapping，外层 wrap 内层
- `evidence` —— 标量诊断字段（不允许嵌套结构）

`evidence` 只接受标量是为了保证错误总能被序列化、写到 JSONL trace、被指标聚合。需要传嵌套结构时序列化为 JSON 字符串塞进去（[scope.py:155-160](../../mutsukicore/core/scope.py#L155-L160) 就是这么处理 `cleanup_failures`）。

### ErrorCode：注册式字符串

[error.py:26-31](../../mutsukicore/contracts/error.py#L26-L31)：

```python
class ErrorCode(RegisteredString):
    _noun = "ErrorCode"
    _unknown_error = UnknownErrorCodeError
    _conflict_error = ErrorCodeConflictError
```

机制和 `CapabilityName` / `PermissionName` 同源（详见 [registered-strings](../05-advanced/registered-strings.md)）。

### 内置 Errs 门面

[error.py:67-105](../../mutsukicore/contracts/error.py#L67-L105)：

| `Errs.*` | 字符串值 | 触发场景 |
|---|---|---|
| `CAPABILITY_NOT_DECLARED` | `capability.not_declared` | 命令需要的 capability 未在 manifest 声明 / 命令不存在 |
| `CAPABILITY_EXHAUSTED` | `capability.exhausted` | `ResourceHost` 资源量纲超限 |
| `SCHEMA_MISMATCH` | `schema.mismatch` | 契约 schema 不兼容 |
| `HANDLE_LEAK` | `handle.leak` | scope 关闭时仍有未释放 handle，或 cleanup 失败 |
| `HANDLE_USE_AFTER_RELEASE` | `handle.use_after_release` | 使用已释放的 handle |
| `REF_CROSS_DOMAIN` | `ref.cross_domain` | RefPayload 跨进程传递（当前 codec 边界未触发） |
| `REF_SERIALIZE_ATTEMPT` | `ref.serialize_attempt` | 试图序列化 RefPayload（当前 codec 边界未触发） |
| `PLUGIN_CYCLE` | `plugin.cycle` | 插件 DAG 存在环 |
| `PLUGIN_SCOPE_VIOLATION` | `plugin.scope_violation` | 副作用未通过 scope 注册（当前主要由 lint / review 捕获） |
| `PLUGIN_DEFINITION_ERROR` | `plugin.definition_error` | 插件定义不合法 / 命令运行时异常 |
| `TRANSACTION_COMPENSATION_FAILED` | `transaction.compensation_failed` | Saga 补偿步骤失败 |
| `AGENT_NOT_FOUND` | `agent.not_found` | 跨 Agent 调用找不到目标 Agent |
| `PERMISSION_DENIED` | `permission.denied` | Permission 检查未通过 |
| `UNKNOWN_CAPABILITY` | `capability.unknown` | 构造未注册的 CapabilityName |
| `SYNC_VIOLATION` | `plugin.sync_violation` | 同步阻塞调用违规 |

`Errs` 用 `bootstrap_facade` 一次性注册：

```python
ErrorCode.bootstrap_facade(
    Errs,
    {
        "CAPABILITY_NOT_DECLARED": "capability.not_declared",
        ...
    },
    declared_by="mutsukicore.core",
)
```

### Command Router 与 Dispatcher 的异常分类

文本命令路径位于 reference extension：[`TextCommandRouterPlugin`](../../mutsukicore_ext/command/__init__.py)。它解析文本、调用 dispatcher，并把结构化错误写成出站消息。命令体经 dispatcher 调用时，handler 抛出的未捕获异常会先由 dispatcher 包成 `Errs.OPERATION_HANDLER_RAISED`。

command router 自己捕获到的非 dispatcher 异常由 `_classify_command_exception(...)` 映射成 `Error`：

| 捕获到的异常 | 映射到 |
|---|---|
| `HandleLeakError` | `Errs.HANDLE_LEAK`，复用其内置 evidence |
| `ServiceNotFoundError` | `Errs.SERVICE_NOT_FOUND`，evidence reason = `service_not_found` |
| `KeyError`（缺参数） | `Errs.COMMAND_INVALID_ARGS`，evidence reason = `missing_arg` |
| 其他 | `Errs.COMMAND_EXECUTION_FAILED`，evidence 包含 `exception_type` / `exception_repr` |

错误 message 写到出站：command router 的 `_emit_error` 把 `Error` 序列化成 `[error <code>] <evidence>` 文本投到 outbox。Operation 执行事实由 dispatcher 的 `dispatch.invoke` trace span 表达；generic envelope consumer 的失败由 `plugin.<id>.on_envelope` span 标记为 ERROR。

### RecoveryAction

[error.py:34-38](../../mutsukicore/contracts/error.py#L34-L38)：

```python
class RecoveryAction(StrEnum):
    RETRY = "retry"
    FALLBACK = "fallback"
    ESCALATE = "escalate"
    ABORT = "abort"
```

由错误的产生方决定建议（"我建议你 retry"），上游决定是否采纳。v0.1 框架自身不消费 RecoveryAction —— 它是给业务层（Yume kernel、tool 调度器）使用。

## 用法示例

构造一个错误：

```python
from mutsukicore.contracts.error import Error, Errs, RecoveryAction

err = Error(
    code=Errs.CAPABILITY_EXHAUSTED,
    source="my-plugin",
    route="command.search",
    recovery=RecoveryAction.FALLBACK,
    evidence={
        "limit": 100,
        "used": 100,
        "window_sec": 60,
    },
)
```

链式 wrap（外层抓到内层错误时不要 stringify，要 chain）：

```python
try:
    await inner_op()
except SomeError as e:
    raise MyAppError(Error(
        code=Errs.PLUGIN_DEFINITION_ERROR,
        source="outer",
        route="command.outer",
        cause=e.error if hasattr(e, "error") else None,
        evidence={"ctx": "outer"},
    )) from e
```

读 chain：

```python
for e in err.chain():
    print(e.code, e.source, e.route)
```

注册自有错误码：

```python
from mutsukicore.contracts.error import ErrorCode

YUME_KERNEL_TIMEOUT = ErrorCode.register(
    "yume.kernel.timeout",
    declared_by="yume.runtime",
)
```

## 常见陷阱

- **`evidence` 只接受标量**——`str | int | float | bool`。要塞 list / dict 必须先 `json.dumps`（参考 [scope.py 的 cleanup_failures_json](../../mutsukicore/core/scope.py#L155-L160) 处理方式）。
- **不要把 `Error` 当 Python 异常抛**。`Error` 是 `Contract`（msgspec.Struct），不是 `Exception`。要抛 → 用一个 wrapper 异常（`HandleLeakError(leaked, error=...)` 这种）携带它。command router 与 dispatcher 期望命令里抛 Python 异常，由它们去分类成 `Error`。
- **`cause` 是 `Error | None`，不是 Python `__cause__`**。用 `raise X from Y` 链接 Python 异常，`Error.cause` 用来链接结构化错误。两者独立。
- **`Errs.*` 在 import 时立即触发注册**。`from mutsukicore.contracts.error import Errs` 这一行就跑了 `bootstrap_facade`。所以 `ErrorCode("permission.denied")` 在 import 之后才能成功；之前会抛 `UnknownErrorCodeError`。
- **避免从旧的 `Errs.PLUGIN_DEFINITION_ERROR` 推断命令运行时原因**。当前命令运行错误分别使用 `service.not_found`、`command.invalid_args`、`command.execution_failed` 或 dispatcher 的 `operation.handler_raised`。要区分原因看 `code` 与 `evidence["reason"]`。
- **错误码字符串不应频繁变化**。它们出现在 alerting 规则、grep 脚本、测试断言里。要废弃一个错误码，先注册新的并迁移调用方，最后再删除旧引用。
