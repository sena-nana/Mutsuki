# Simple ReAct

`mutsuki_agent_adapter_api::SimpleReact` 是开箱即用的 **model ↔ tool** 循环。

它从完整 `mutsuki.agent/run@1` agent loop 抽出最小对话逻辑：调用模型 → 若有 tool
calls 则执行工具 → 将 tool result 写回消息 → 重复，直到模型结束或 `max_steps`
耗尽。

## 使用

```rust
use std::sync::Arc;
use mutsuki_agent_adapter_api::{
    FnToolExecutor, SimpleReact, SimpleReactRequest, tool_result_message,
};
use mutsuki_agent_contracts::AgentMessage;

let react = SimpleReact::new(adapter, provider, tools, Arc::new(FnToolExecutor::new(
    |call| Box::pin(async move { tool_result_message(&call, "ok") }),
)))?;

let result = react
    .run(SimpleReactRequest::new("model", vec![AgentMessage::user("hi")]).with_max_steps(8))
    .await?;
```

## 包含 / 不包含

| 包含 | 不包含 |
| --- | --- |
| 多轮 model generate | Session 持久化 / fork |
| Tool 因果 `call_id` | Approval 审批 UI |
| Usage 累计与 max_steps | 记忆 / 知识 / sub-agent |
| 任意 `ModelProtocolAdapter` | Host TaskPool 装配细节 |

需要 session、审批、budget 事件流、context assembly 时，使用完整 agent loop
（`mutsuki-plugin-agent-loop`）与产品 Host 装配，而不是扩展 SimpleReAct 为第二套
runtime。
