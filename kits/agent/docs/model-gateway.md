# Model Gateway

`mutsuki.agent.model/generate@1` 与 `.../stream@1` 是 provider-neutral Domain
协议，由 `ModelAsyncHandler`（`AsyncBatchHandler`）在 TaskPool 上执行。生产 gateway
默认不注册任何 Provider，缺失显式注入时 fail loud；deterministic mock provider 只由
`mutsuki-agent-testkit` 提供。消费端通过 `HttpModelProviderOptions` 显式构造
provider，并在构造时注入 credential；AgentKit 不读取环境变量、配置文件或 Secret
backend。

HTTP 请求只允许走 generate/stream 的 cancellable future 路径。
`HttpModelProvider.generate()` 与 Adapter-backed provider 一样拒绝 inline 调用
（`agent.model.effect_runner_required`），避免绕过 cancel/timeout。

契约仍保留 `effect.mutsuki.agent.model/http@1`，但 Model Gateway runner 是
`RunnerPurity::Pure` / `ProtocolClass::Domain`，不能同时 accept `effect.*`。
HTTP I/O 发生在 `generate_async` / `stream_async` 内部，而不是一条可单独提交的
effect 协议。独立 effect runner 若需要再引入，必须是 `RunnerPurity::Effectful`。

`mutsuki.agent.model/poll@1` 保留在契约和 manifest 上，但当前 handler 会
fail-loud：async handler 在 generate/stream 完成时已经结束 HTTP 调用，没有独立
poll 子 task。

stream 同时返回 `ResourceRef`（大 payload）和 assistant `message`（transcript 续写）。
Loops 必须把 `message` 写回 durable transcript，不能用空 assistant 续跑。
