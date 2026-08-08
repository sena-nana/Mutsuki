# Model protocol adapters

三种基础 LLM 协议，统一实现 `ModelProtocolAdapter`：

| Adapter id | Protocol | Type | Endpoint |
| --- | --- | --- | --- |
| `openai-compatible` | `openai.chat-completions` | `OpenAiCompatibleAdapter` | `/v1/chat/completions` |
| `openai-responses` | `openai.responses` | `OpenAiResponsesAdapter` | `/v1/responses` |
| `anthropic-messages` | `anthropic.messages` | `AnthropicMessagesAdapter` | `/v1/messages` |

**负责：** 协议映射、tool 因果链、错误分类、基本对话与 `SimpleReact`（model↔tool）。  
**不负责：** 子代理、记忆/知识、审批 UI、产品 Persona/工作流、Secret 读取。

## Simple ReAct

```rust
use mutsuki_agent_adapter_api::{FnToolExecutor, SimpleReact, SimpleReactRequest, tool_result_message};

let react = SimpleReact::new(adapter, provider, tools, Arc::new(FnToolExecutor::new(
    |call| Box::pin(async move { tool_result_message(&call, "ok") }),
)))?;
let result = react.run(SimpleReactRequest::new("model", messages).with_max_steps(8)).await?;
```

需要 session / 审批 / context assembly 时用完整 `mutsuki.agent/run@1` loop，不要把 SimpleReAct 扩成第二套 runtime。
