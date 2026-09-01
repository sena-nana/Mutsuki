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

运行时边界同时读取 `../../AGENTS.md`；Host 装配读取
`../../hosts/service/AGENTS.md`。

## Hard Rules

1. 业务插件默认只依赖 `mutsuki.bot.*`；平台字段和行为留在平台命名空间或 Adapter 内部。
2. 不创建 BotHost/QQBotHost；常驻生命周期归 ServiceHost，桌面生命周期归 TauriHost。
3. Runner 只走 batch-first `run_batch`，每个 entry 独立完成；task 提交、取消和 outcome 使用 `TaskHandle`。
4. socket、HTTP client、SDK client、数据库连接和媒体字节不得跨 runtime 边界；大数据使用资源 descriptor。
5. token/secret 由 Host key 引用和注入，不进入 manifest、示例、fixture、日志或提交配置。
6. manifest、RunnerDescriptor、EventSource 和 LoadPlan capability 必须与真实实现一致；缺失时 fail loud。
7. 禁止复制 Core/Host/Agent 实现、生产 fallback 或兼容 shim。
8. 仓内 Mutsuki 依赖必须继承根 Workspace 的 path；禁止内部 Git pin、仓库外 Cargo `path`
   和本地 `[patch]`。WebHost 与 Bot package 必须在同一 release revision 原子验证。
9. 平台 Adapter crate 不依赖具体 Host；`HostEventSource`、health 和 builder 安装只能位于显式 integration crate。
10. 媒体等可选后端必须显式提供并与 manifest capability 一致，不注册 unavailable 生产替代。
11. QQBot 文档必须区分单元、fake E2E 和真实账号 smoke，且与当前 manifest、配置和实现同步。
12. 有 `PluginBuilder` 的可加载面必须使用 `mutsuki-plugin-*` 名；库面 crate 只持有 trait/service。
    `mutsuki-bot-state-db` 实现 conversation/persona/delivery/interaction/sandbox 库面 store，
    禁止反向依赖 plugin 包。
13. Flow 是业务行为唯一启动面：业务插件只经图节点 binding 被调用，或只提交
    `mutsuki.bot.flow/ingress@1` 触发事件并到此为止。业务 EventSource 的提交面必须经
    `mutsuki-bot-sdk` 的 `BotSubmissionGate` 包装（拒绝直提 `message/send`、`message/recall`、
    `delivery/*`、`agent/*`），业务 manifest 注册前必须通过
    `BotSubmissionGate::ensure_manifest_business_surface` 校验。活动图未接线等于对应业务冻结；
    冻结经 ingress 统计（`accepted_total`/`dropped_total`）与 `mutsuki.bot.flow.ingress`
    健康探针可观测，不是静默失败。插件经 Flow 拥有的两个接口查询自身连线状态：
    `BotNodeInvocation.wiring`（随图版本固定的端口级连线）与 `mutsuki.bot.flow.registry`
    host service 的 `node_wiring`/`source_wired`；业务推送管线据此在未接线时跳过上游工作
    （Bilibili 轮询跳过上游 API 并基线化游标，不回放冻结窗口）。
14. 已发起的效果经持久完成路径排空：`BotReplyDeliveryRecoveryEventSource`、reserved draft
    Submit、interaction waiter 以及 adapter/delivery 服务本身不在第 13 条限制内；控制面
    （管理 API、Web 控制台）同样豁免。图外直连 `mutsuki.bot.agent/submit@1` 仅保留给测试面，
    不是生产启动路径。

完整 crate 表见 `docs/architecture.md`。关键边界：

| crate | 职责 |
| --- | --- |
| `mutsuki-bot-protocol` | 纯契约。`event/ingest`、`command/handle` 是 envelope ID，不是 runner |
| `mutsuki-bot-conversation` / `mutsuki-bot-persona` | 会话与 persona 的 store trait；plugin 包只做 Runner |
| `mutsuki-bot-interaction` / `mutsuki-bot-delivery` | waiter / delivery 服务与 repository；`DeliveryGateway` 用 `BotTarget` |
| `mutsuki-plugin-bot-interaction` / `mutsuki-plugin-bot-delivery` | 对应 PluginBuilder manifest 与节点 catalog |
| `mutsuki-bot-state-db` | SQLite 实现上述库面 store |
| `mutsuki-bot-service-host-integration` | 显式 Host 装配面；禁止再往里加业务 Runner |
| `mutsuki-bot-web-console` | Bot 包提供的 WebHost 装配 helper，产品可选启用 |

## 验证

Rust 改动运行 `cargo fmt --check`、`cargo check` 和 `cargo test`。平台和装配改动补充
外部边界 fake 或 smoke；最终报告实际命令、测试层级和统一 release revision。

提交前检查 `git status --short` 和定向 diff，提交标题使用中文短句。
