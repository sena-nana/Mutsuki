# Model Gateway

`mutsuki.agent.model/generate@1` 与 `.../stream@1` 是 provider-neutral orchestration
协议。生产 gateway 默认不注册任何 Provider，缺失显式注入时 fail loud；deterministic
mock provider 只由 `mutsuki-agent-testkit` 提供。真实 HTTP provider 由
`effect.mutsuki.agent.model/http@1` runner 执行，支持异步取消、硬超时、错误映射和
有限重试。消费端通过 `HttpModelProviderOptions` 显式构造 provider，并在构造时注入
credential；AgentKit 不读取环境变量、配置文件或 Secret backend。

产品装配层在其 Tokio runtime 上运行可中止的 HTTP future，并用
`mutsuki.agent.model/poll@1` 子 task 形成 Core `TaskAwait`。cancel、reload 或 runner
drop 会 abort 对应请求；无需修改 Core scheduler。

stream task/event 仅返回 `ResourceRef`；正文保存在 `ModelGateway` 的 provider 资源
存储中，不通过普通 task payload 搬运。
