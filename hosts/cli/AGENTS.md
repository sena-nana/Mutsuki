# MutsukiCliHost 工作规范

本仓库是 MutsukiServiceHost 的独立命令行和终端 UI 客户端。它通过公开、带鉴权的
控制协议展示并操作 service/core/plugin/runner/task/log/health 状态，不嵌入 Core，
也不实现插件、Runner Kit、SDK 或领域业务。

## 技能路由

- `skills/control-client/SKILL.md`：控制协议、IPC、认证、请求和状态模型。
- `skills/terminal-ui/SKILL.md`：ratatui/crossterm 界面、输入和会话生命周期。
- `skills/integration-testing/SKILL.md`：真实 ServiceHost 控制面、错误和断线验收。

控制协议变更先读取 `../MutsukiServiceHost/AGENTS.md` 及其 control-api 技能。

## Hard Rules

1. CLI/TUI 只消费公开控制 API，不直接访问 Core handle、进程、secret 或插件内部状态。
2. ServiceHost 管守护进程和运行环境；本仓库只管理交互式终端客户端生命周期。
3. UI 状态必须来自真实响应；后端缺失或断线时明确 unavailable，不伪造 available/healthy。
4. task 取消和 outcome 使用控制协议中的 `TaskHandle` 语义，不建立并行业务路径。
5. 不实现 Agent、Bot、Provider、会话模拟、业务插件、生产 fallback 或兼容 shim。
6. token 只用于认证，不进入界面、普通日志或错误文本。
7. 禁止仓库外 Cargo `path`/本地 `[patch]`；跨仓库依赖使用远端 Git URL 和固定 `rev`。
8. CLI 只构造公开 `ControlClient`；不得加载或复用服务端 `ServiceConfig`、profile 和 secret backend。

## 验证

Rust 改动运行 `cargo fmt --check`、`cargo check` 和 `cargo test`。控制面改动优先用真实
ServiceHost 或协议级 fake 验证状态转换、鉴权和断线行为；最终报告实际命令与结果。
