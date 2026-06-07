# API · `mutsukibot.core.dispatcher`

Dispatcher 是 Operation / Source 路由入口。

## 公开符号

| 符号 | 说明 |
|---|---|
| `Dispatcher` | 单 Agent 的 Operation/Source 表 |
| `OperationInvokeError` | invoke / publish 失败的结构化载体 |
| `OperationStatus` | `active` / `unhealthy` / `unregistering` |
| `SourceStatus` | `active` / `unregistering` |
| `OperationConflictError` | 同一 `op_id` 冲突 |
| `SourceConflictError` | 同一 `source_id` 冲突 |
| `OperationUndeclaredError` | runtime 注册未在 manifest 声明 |
| `SourceUndeclaredError` | runtime 注册未在 manifest 声明 |

## 用法

```python
ctx.dispatch.register_operation(...)
ctx.dispatch.register_source(...)
await ctx.dispatch.invoke("backend:default.notify", {"message": "observed"}, ctx=ctx)
await ctx.dispatch.invoke_in_agent("agent-b", "memory:agent-b.recall", {}, ctx=ctx)
await ctx.dispatch.publish(envelope)
```

`invoke` 与 `invoke_in_agent` 都是 inline await，不进 queue。`invoke_in_agent` 显式指定目标 Agent，不隐式广播；目标不存在时抛 `OperationInvokeError(error.code == Errs.AGENT_NOT_FOUND)`。`publish` 会广播给所有 `Agent.accepts` 匹配的 awake Agent。
