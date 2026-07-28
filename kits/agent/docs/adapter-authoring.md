# Protocol Adapter authoring

An Adapter maps the unified Agent model request and streaming events to one provider protocol. It is
not a provider daemon and does not own scheduling.

1. Publish a `ModelProtocolAdapterDescriptor` with supported capabilities and compatibility data.
2. Describe each configured `ProviderInstanceDescriptor` separately from the Adapter brand.
3. Implement generation/streaming as a Mutsuki `AsyncBatchHandler`.
4. Preserve typed tool calls, finish reasons, usage, retry classification and cancellation.
5. Return large or streaming bodies through `ResourceRef`; keep only summaries in messages.
6. Let Host policy resolve endpoint, timeout, transport and `CredentialRef`.

`mutsuki-agent-adapter-openai` is the reference Chat Completions implementation. It accepts an
injected transport, maps tools and structured output, bounds retries/timeouts, and drops the
in-flight transport future on cancellation. It does not read environment variables or ship a
default credential.

`mutsuki-agent-adapter-anthropic` is the Anthropic Messages counterpart (`x-api-key`,
`anthropic-version`, `/v1/messages`). It is generate-first in the current slice; Hosts inject
`CredentialRef` and provider endpoints the same way as the OpenAI-compatible Adapter.

Breaking DTO/wire changes require a contract major-version change. Provider endpoint, brand or
model-catalog changes belong to instance configuration and must not break the authoring API.
