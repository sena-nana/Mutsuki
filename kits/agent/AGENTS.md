# MutsukiAgentKit 工作规范

本仓库拥有 Rust 原生 Agent 协议、SDK/宏、AgentLoop、上下文、会话、工具、记忆、
模型网关、Prompt 插件和测试工具。它不拥有 Core 调度、Host 生命周期、Bot 平台适配、
Python Runner SDK 或具体产品装配。

## 阅读顺序与技能路由

先读 `README.md`、`docs/architecture.md`、`docs/protocol.md` 和相关 crate/test：

- `skills/protocol/SKILL.md`：Agent DTO、协议 ID、schema 和 manifest surface。
- `skills/sdk/SKILL.md`：Rust SDK、typed client、builder 和 proc macro。
- `skills/plugins/SKILL.md`：AgentLoop、context/session、tool/memory/model/prompt 插件。
- `skills/testkit/SKILL.md`：conformance、fake provider/tool、bundle、模板和示例。

涉及 runtime contract 时同时读取 `../MutsukiCore/AGENTS.md`。

## Hard Rules

1. 协议 DTO 只放 protocol crate；SDK 只包装协议；宏不创建第二套工具 runtime。
2. Agent 插件通过 Mutsuki task/Runner 路由交互，不能用隐藏直调替代 runtime 行为。
3. Runner 使用 batch-first `run_batch`，task 操作使用 `TaskHandle`，并传播 trace/correlation/generation。
4. 模型 Provider、工具和记忆实现属于 AgentKit 或独立 Provider 仓库；Host 只装配并注入 secret。
5. 不实现 Bot Adapter、Host 控制面、Core TaskPool 或 Python Runner backend。
6. manifest、schema、SDK marker 和真实 Runner 必须同步；缺失 backend/capability 时 fail loud。
7. 禁止占位公开能力、复制上游实现、生产 fallback 或兼容 shim。
8. 禁止仓库外 Cargo `path`/本地 `[patch]`；跨仓库依赖使用远端 Git URL 和固定 `rev`。
9. effectful 模型 Provider 只能由模型 effect Runner 调用；普通 ModelGateway inline API 必须拒绝网络 Provider。

## 验证

Rust 改动运行 `cargo fmt --check`、`cargo check` 和 `cargo test`。协议或插件 surface
改动补充行为测试和 conformance；最终报告实际命令与远端 revision。
