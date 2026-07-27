# Rust SDK

基于 `RuntimeClient` / `TaskSubmitter` / `TaskHandle` / `AsyncRunnerContext`，不自建 scheduler。

- `AgentClient` / `ModelClient` / `MemoryClient`：typed protocol 调用（自动透传 trace）
- `with_trace`：提交前附加 `trace_id` / `correlation_id`
- `orchestration_runner` / `effectful_runner`：batch-first descriptor（batch/payload/resources/ordering/control）
- `#[agent_tool]`：生成 tool descriptor，不创建 runtime
