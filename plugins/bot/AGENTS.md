# MutsukiBotPlugins 工作规范

本仓库拥有 Mutsuki Bot 领域协议、Rust SDK、通用事件/命令 Runner 和平台
Adapter/Gateway。它不拥有 Core 调度、Host 生命周期、Agent 能力或产品装配。

## 阅读顺序与技能路由

先读 `README.md`、`docs/architecture.md`、`docs/protocol.md` 和相关 crate/test，再按方向读取：

- `skills/bot-protocol-sdk/SKILL.md`：`mutsuki.bot.*` DTO、协议和 SDK。
- `skills/event-routing-command/SKILL.md`：事件路由、订阅、命令解析和 dispatch。
- `skills/platform-adapters/SKILL.md`：QQBot 等平台 Adapter、Gateway 和 transport。
- `skills/service-host-integration/SKILL.md`：bundle、manifest、EventSource 和 ServiceRuntime 装配。
- `skills/bot-testing/SKILL.md`：batch Runner、fake transport、闭环和真实 smoke。
- `skills/qqbot-documentation/SKILL.md`：QQBot 配置、能力矩阵、官方协议核对、运行与排障文档。

运行时边界同时读取 `../MutsukiCore/AGENTS.md`；Host 装配读取
`../MutsukiServiceHost/AGENTS.md`。

## Hard Rules

1. 业务插件默认只依赖 `mutsuki.bot.*`；平台字段和行为留在平台命名空间或 Adapter 内部。
2. 不创建 BotHost/QQBotHost；常驻生命周期归 ServiceHost，桌面生命周期归 TauriHost。
3. Runner 只走 batch-first `run_batch`，每个 entry 独立完成；task 提交、取消和 outcome 使用 `TaskHandle`。
4. socket、HTTP client、SDK client、数据库连接和媒体字节不得跨 runtime 边界；大数据使用资源 descriptor。
5. token/secret 由 Host key 引用和注入，不进入 manifest、示例、fixture、日志或提交配置。
6. manifest、RunnerDescriptor、EventSource 和 LoadPlan capability 必须与真实实现一致；缺失时 fail loud。
7. 禁止复制 Core/Host/Agent 实现、生产 fallback 或兼容 shim。
8. 禁止仓库外 Cargo `path`/本地 `[patch]`；跨仓库依赖使用远端 Git URL 和固定 `rev`。
   WebHost（`mutsuki-web-host` / `mutsuki-web-protocol`）必须用 Git `rev` pin，不得绑成仓内
   path；产品组合以 BotTemplate release-set 的 `web_host` 为权威，可独立 bump。
9. 平台 Adapter crate 不依赖具体 Host；`HostEventSource`、health 和 builder 安装只能位于显式 integration crate。
10. 媒体等可选后端必须显式提供并与 manifest capability 一致，不注册 unavailable 生产替代。
11. QQBot 文档必须区分单元、fake E2E 和真实账号 smoke，且与当前 manifest、配置和实现同步。

## 验证

Rust 改动运行 `cargo fmt --check`、`cargo check` 和 `cargo test`。平台和装配改动补充
外部边界 fake 或 smoke；最终报告实际命令、测试层级和远端 revision。

提交前检查 `git status --short` 和定向 diff，提交标题使用中文短句。
