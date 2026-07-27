# MutsukiBotTemplate 工作规范

本仓库是 **配置驱动的 Mutsuki Bot 产品模板**，也是 Bot 需求进入多仓库体系后的
能力边界核查入口。它只负责外部配置契约、catalog 聚合、产品装配和跨仓库验收，
不拥有 Core、Host、Bot、Agent 或平台能力的实现。

## 阅读顺序

1. 当前及关联 issue，确认目标、依赖和验收场景。
2. `../MutsukiCore/AGENTS.md` 与 `plans/{roadmap,architecture,engineering,contracts}.md`。
3. 候选依赖仓库的 `AGENTS.md`、公开 API、manifest 和测试。
4. 本文件路由的相关技能，再检查当前实现、远端 commit 和 lockfile。

Issue 是需求线索，不是当前 API 的事实源。存在 `.codegraph/` 时，定位代码先用 CodeGraph。

## 技能路由

- `skills/capability-boundaries/SKILL.md`：判断能力归属和跨仓库顺序。
- `skills/remote-dependencies/SKILL.md`：Git 依赖、远端 pin、lockfile 和独立 checkout。
- `skills/bot-assembly/SKILL.md`：配置契约、LoadPlan 和 ServiceRuntime 装配。
- `skills/integration-testing/SKILL.md`：mock、fake server、真实 smoke、health 和 shutdown。

职责不明先读 capability-boundaries；涉及依赖同时读 remote-dependencies。

## 职责边界

| 仓库 | 职责 |
| --- | --- |
| `MutsukiCore` | 领域中立 contracts、Task/Runner、资源、LoadPlan 和 Rust Host/SDK 基础面 |
| `MutsukiStdPlugins` | 领域中立标准协议，以及 config/db/fs/http/observe/resource/workflow 插件 |
| `MutsukiPythonRunnerKit` | Core Runner Link 的 Python contract mirror、Runner backend、transport 和测试工具 |
| `MutsukiServiceHost` | 服务生命周期、配置/secret、插件加载、EventSource、控制面和 health |
| `MutsukiBotPlugins` | `mutsuki.bot.*` 协议、Bot SDK、标准 Runner、平台 Adapter/Gateway 和显式 Host integration crate |
| `MutsukiAgentKit` | Agent 协议、SDK、模型、工具和记忆能力 |
| `MutsukiCliHost` | ServiceHost 公开控制 API 的 CLI/TUI 客户端 |
| `MutsukiTauriHost` | 内嵌 Core 的桌面 Host、Tauri/WebView bridge 和前端 SDK |
| `MutsukiWebHost` | Web 运行宿主：HTTP/WS、静态资源、RPC/Event bridge、WebExtension 加载与 Recovery Shell |
| 其他能力仓库 | 自己领域的协议、插件、Provider、Runner 或 sidecar |
| 本仓库 | 外部配置契约、catalog 聚合、ServiceRuntime 启动和跨仓库装配验收 |

## Hard Rules

1. 能力缺失时在所属仓库补齐、验证并推送，再更新模板；禁止复制实现、生产 fallback 或兼容 shim。
2. 禁止提交指向仓库外的 Cargo `path`/`[patch]`；跨仓库 Git 依赖固定 `rev`，且远端必须可独立解析。仓库内 member path 不受此限。
3. 配置只声明 capability、插件和部署选择。模板不按平台、Agent、Provider 或 backend 硬编码替代路径。
4. 只提交无账号、无凭据的简单 `config/template.toml` 与 Secret 占位模板；不暴露完整 Host 高级配置样例。实际 `config/local.toml`、账号和专用 secret 文件只能本地存在并被 Git 忽略，主配置只保存 Host secret key 引用。
5. 模板不得拥有业务 Runner、命令、回复或 Agent 流程；这些能力由 owner 仓库实现，并遵守 batch-first、`TaskHandle` 和通用协议契约。
6. RuntimeProfile/RuntimeLoadPlan 是装配权威；registry freeze 后不得动态越权注册。
7. 缺失 capability、配置、secret、artifact 或 revision 必须结构化失败，禁止假成功和吞错。
8. 生产入口按 CLI、`MUTSUKI_CONFIG`、仓库 `config/local.toml` 的顺序选择配置；脚手架只公开用户需要修改的产品字段，目录、IPC、Runner 和观测等高级值继承 ServiceHost 默认。生产代码只聚合 owner factory catalog，不默认启用平台、Router、Command、Agent、Provider 或业务插件。零插件配置可以启动为空闲 Runtime；显式选择的能力缺失时必须失败。Mock 仅限测试。

## Git 与验证

- 工作前后检查 `git status --short`；跨仓库先提交并推送上游，再提交模板 pin。
- Rust 或依赖改动运行 `cargo fmt --check`、`cargo check`、`cargo test`。
- 装配或依赖改动再运行 `cargo metadata --locked`，并在没有兄弟仓库的独立 checkout 验证。
- 最终说明列出实际命令和结果；测试断言行为，不只匹配日志或文案。
