# MutsukiBotTemplate 工作规范

本目录是 **配置驱动的 Mutsuki Bot 产品模板规范源**，也是 Bot 需求进入 monorepo 后的
能力边界核查入口。它只负责外部配置契约、catalog 聚合、产品装配和跨 package 验收，
不拥有 Core、Host、Bot、Agent 或平台能力的实现。

## 阅读顺序

1. 当前及关联 issue，确认目标、依赖和验收场景。
2. 在主仓中读取 `../../AGENTS.md` 与
   `../../plans/{roadmap,architecture,engineering,contracts}.md`；独立生成仓只回到
   `sena-nana/Mutsuki` 提交变更。
3. 候选依赖仓库的 `AGENTS.md`、公开 API、manifest 和测试。
4. 本文件路由的相关技能，再检查当前实现、远端 commit 和 lockfile。

Issue 是需求线索，不是当前 API 的事实源。存在 `.codegraph/` 时，定位代码先用 CodeGraph。

## 技能路由

- `skills/capability-boundaries/SKILL.md`：判断能力归属和跨仓库顺序。
- `skills/template-export/SKILL.md`：从规范源导出独立模板、统一 release pin 和 clean clone。
- `skills/bot-assembly/SKILL.md`：配置契约、LoadPlan 和 ServiceRuntime 装配。
- `skills/integration-testing/SKILL.md`：mock、fake server、真实 smoke、health 和 shutdown。

职责不明先读 capability-boundaries；涉及导出或依赖同时读 template-export。

## 职责边界

| package 目录 | 职责 |
| --- | --- |
| `crates/*` | 领域中立 contracts、Task/Runner、资源、LoadPlan、Link 和 Rust Host/SDK 基础面 |
| `plugins/std` | 领域中立标准协议，以及 config/db/fs/http/observe/resource/workflow 插件 |
| `kits/python-runner` | Runner Link 的 Python contract mirror、Runner backend、transport 和测试工具 |
| `hosts/service` | 服务生命周期、配置/secret、插件加载、EventSource、控制面和 health |
| `plugins/bot` | `mutsuki.bot.*` 协议、Bot SDK、标准 Runner、平台 Adapter/Gateway 和显式 Host integration crate |
| `kits/agent` | Agent 协议、SDK、模型、工具和记忆能力 |
| `hosts/cli` | ServiceHost 公开控制 API 的 CLI/TUI 客户端 |
| `hosts/tauri` | 内嵌 Core 的桌面 Host、Tauri/WebView bridge 和前端 SDK |
| `hosts/web` | Web 运行宿主：HTTP/WS、静态资源、RPC/Event bridge、WebExtension 加载与 Recovery Shell |
| `hosts/distributed` | 分布式控制面、调度、恢复和资源预算 |
| 外部业务仓库 | 自己领域的协议、插件、Provider、Runner 或 sidecar |
| 本目录 | 外部配置契约、catalog 聚合、ServiceRuntime 启动和跨 package 装配验收 |

## Hard Rules

1. 能力缺失时在 owner package 补齐并验证，再更新模板；禁止复制实现、生产 fallback 或兼容 shim。
2. 规范源使用根 Workspace path；导出的独立模板必须把依赖转换为统一 Mutsuki tag/commit，禁止仓库外 `path`/`[patch]`。
3. 配置只声明 capability、插件和部署选择。模板不按平台、Agent、Provider 或 backend 硬编码替代路径。
4. 只提交无账号、无凭据的最小 `config/bootstrap.toml` 与 Secret 占位模板；bootstrap 只选择 Host 边界与配置仓库，产品和 owner 配置进入 `ConfigRepository`，只保存 secret key 引用。
5. 模板不得拥有业务 Runner、命令、回复或 Agent 流程；这些能力由 owner 仓库实现，并遵守 batch-first、`TaskHandle` 和通用协议契约。
6. RuntimeProfile/RuntimeLoadPlan 是装配权威；registry freeze 后不得动态越权注册。
7. 缺失 capability、配置、secret、artifact 或 revision 必须结构化失败，禁止假成功和吞错。
8. 生产入口按 CLI、`MUTSUKI_BOOTSTRAP`、仓库 `config/bootstrap.toml` 选择最小 bootstrap；模板显式选择 SQLite 配置仓库，但框架不假设路径或存储实现。空仓库只写一次版本化种子，不启用 Runtime 插件且只选择通用配置 WebExtension；其他插件和编辑器均由保存后的产品配置显式启用。Mock 仅限测试。
9. `sena-nana/MutsukiBotTemplate` 只是 release 自动生成的 GitHub Template；禁止在生成仓
   手工维护实现、规范、Issue 或依赖版本。

## Git 与验证

- 工作前后检查 `git status --short`；owner package 与模板源在同一 revision 原子提交。
- Rust 或依赖改动运行 `cargo fmt --check`、`cargo check`、`cargo test`。
- 装配或依赖改动再运行 `cargo metadata --locked`，并验证导出的独立模板。
- 最终说明列出实际命令和结果；测试断言行为，不只匹配日志或文案。
