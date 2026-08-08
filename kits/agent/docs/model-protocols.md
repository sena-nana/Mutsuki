# Model protocol adapters

AgentKit 提供三种基础 LLM 协议接口。三者都实现统一的 `ModelProtocolAdapter`
（`generate` / `stream`），映射到同一套 `AgentMessage` / `AgentToolCall` 契约。

| Adapter id | Protocol family | Crate / type | Endpoint |
| --- | --- | --- | --- |
| `openai-compatible` | `openai.chat-completions` | `OpenAiCompatibleAdapter` | `/v1/chat/completions` |
| `openai-responses` | `openai.responses` | `OpenAiResponsesAdapter` | `/v1/responses` |
| `anthropic-messages` | `anthropic.messages` | `AnthropicMessagesAdapter` | `/v1/messages` |

## 职责边界

**Mutsuki 负责**

- 协议请求/响应映射（消息、tools、stop reason、usage）
- tool call 与 tool result 的因果链校验
- 错误分类（retryable / auth / rate limit / protocol）
- 基本对话与可开箱的简单 ReAct（见 [`simple-react.md`](simple-react.md)）

**Mutsuki 不负责**

- 子代理编排、记忆路由、知识库、proactive
- 产品 Persona / 工作流 / 审批 UI
- 分布式 checkpoint 的产品用法
- 读取环境变量或内置 Secret

Host / 产品注入 `ProviderInstanceDescriptor`、`CredentialBroker` 与 endpoint。

## 统一入口

```rust
use mutsuki_agent_adapter_api::ModelProtocolAdapter;
use mutsuki_agent_adapter_openai::{OpenAiCompatibleAdapter, OpenAiResponsesAdapter};
use mutsuki_agent_adapter_anthropic::AnthropicMessagesAdapter;

// 任一 adapter 均可通过 ModelProtocolAdapter::generate / stream 调用。
```

Breaking wire 变更需要 contract major；endpoint / model catalog 变更属于 instance 配置。
