# Protocol Adapter authoring

An Adapter maps the unified Agent model request and streaming events to one provider protocol. It is
not a provider daemon and does not own scheduling.

1. Publish a `ModelProtocolAdapterDescriptor` with supported capabilities and compatibility data.
2. Describe each configured `ProviderInstanceDescriptor` separately from the Adapter brand.
3. Implement generation/streaming as a Mutsuki `AsyncBatchHandler`.
4. Preserve typed tool calls, finish reasons, usage, retry classification and cancellation.
5. Return large or streaming bodies through `ResourceRef`; keep only summaries in messages.
6. Let Host policy resolve endpoint, timeout, transport and `CredentialRef`.

AgentKit ships three basic protocol adapters (see [`model-protocols.md`](model-protocols.md)):

| Adapter | Protocol | Notes |
| --- | --- | --- |
| `OpenAiCompatibleAdapter` | `openai.chat-completions` | Chat Completions; tools, structured output, SSE stream |
| `OpenAiResponsesAdapter` | `openai.responses` | Responses API (`/v1/responses`); function_call causal chain |
| `AnthropicMessagesAdapter` | `anthropic.messages` | Messages API (`x-api-key`, `anthropic-version`); generate-first |

All accept an injected `CredentialBroker` and Host-provided endpoint. They do not read environment
variables or ship a default credential. Retries/timeouts come from
`ProviderInstanceDescriptor.compatibility`.

For a minimal model↔tool loop without session/approval, use
`SimpleReact`（见 model-protocols.md） over any `ModelProtocolAdapter`.

Breaking DTO/wire changes require a contract major-version change. Provider endpoint, brand or
model-catalog changes belong to instance configuration and must not break the authoring API.
