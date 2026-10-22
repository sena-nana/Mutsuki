# MutsukiBotPlugins 工作规范

本目录拥有 Mutsuki Bot 领域协议、Rust SDK、通用事件/命令 Runner 和平台
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
   连接与轮询逻辑本身属于 owner：QQ Gateway WebSocket 客户端是 adapter 的 `gateway::QqGatewayClient`，
   Bilibili 轮询是 `run_bilibili_polling`；integration crate 只做密钥、descriptor、健康映射和生命周期包装。
10. 媒体等可选后端必须显式提供并与 manifest capability 一致，不注册 unavailable 生产替代。
11. QQBot 文档必须区分单元、fake E2E 和真实账号 smoke，且与当前 manifest、配置和实现同步。
12. 有 `PluginBuilder` 的可加载面必须使用 `mutsuki-plugin-*` 名；库面 crate 只持有 trait/service。
    插件 ID 一律为 `mutsuki.plugin.bot.<crate 后缀>`（如 `mutsuki.plugin.bot.adapter-qqbot`、`mutsuki.plugin.bot.flow`、
    `mutsuki.plugin.bot.sandbox`）；host service ID 保持 `mutsuki.bot.*` 且不得与插件 ID 复用，Flow 节点类型 ID 不随改名变化。
    反向同样成立：不建 `PluginBuilder` 的 crate 不得占用该前缀。WebExtension 属于此类，
    统一用 `mutsuki-bot-web-extension-*`，且骨架从 `mutsuki-web-extension-api` 复用。
    不含 Bot 内容的控制台页面归各自 owner（`hosts/web`、`kits/agent`），不留在本包。
    `mutsuki-bot-state-db` 实现 conversation/persona/delivery/interaction/sandbox 库面 store，
    禁止反向依赖 plugin 包。`PersonaStore` 与 `ConversationContextStore` 同属
    `mutsuki-bot-conversation`；plugin crate 不得 re-export 库面 trait，消费者一律从 owner 引入。
    内存实现是测试替身，只住在 `mutsuki-bot-testkit`，不得出现在生产库 crate 的公开面。
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
    不是生产启动路径。该豁免由 `BotManifestSurface` 三档显式表达，注册处按档调用
    `BotSubmissionGate::ensure_manifest_surface`，不得跳过校验：
    - `Business`：图内业务节点，禁止 `message/send`、`message/recall`、`mutsuki.bot.delivery/*`
      与 `mutsuki.bot.agent/*`。
    - `DurableReplyProducer`：把已产出的回复交给持久投递服务，可声明
      `mutsuki.bot.delivery/*`，仍禁止直发平台与发起 Agent 轮次。Agent bridge 属于此档。
    - `EffectDrain`：真正执行外部发送的 delivery/adapter 服务，可声明平台
      `send`/`recall` 与 `delivery/*`，仍禁止发起 Agent 轮次。
    三档均禁止 `mutsuki.bot.agent/*`：Agent 轮次只能由图节点 binding 发起。
    Flow Router 自身是发起面，为每个节点 binding 声明 `requires`，不适用该校验。

完整 crate 表见 `docs/architecture.md`。关键边界：

| crate | 职责 |
| --- | --- |
| `mutsuki-bot-protocol` | 纯契约。`event/ingest`、`command/handle` 是 envelope ID，不是 runner；平台中立的 `BotConversationRef`、`BotAdapterDescriptor`/`BotAdapterDirectory` 与 `BotAdapterCapabilities` |
| `mutsuki-bot-console-protocol` | 控制台 WebExtension 的纯 DTO 与 `TypedRpc` 标记（如 `platforms`）；前端契约由其生成 |
| `mutsuki-bot-qq-protocol` | QQ 平台契约：`mutsuki.bot.qqbot.*` 协议 ID、`qqbot.openapi.*` 错误码、`QqBotCapabilityMatrix`；业务插件不得依赖 |
| `mutsuki-bot-conversation` | 会话、ICL 与 persona 的 store trait；plugin 包只做 Runner |
| `mutsuki-bot-interaction` / `mutsuki-bot-delivery` | waiter / delivery 服务与 repository；`DeliveryGateway` 用 `BotTarget` |
| `mutsuki-plugin-bot-interaction` / `mutsuki-plugin-bot-delivery` | 对应 PluginBuilder manifest 与节点 catalog |
| `mutsuki-bot-state-db` | SQLite 实现上述库面 store；产品装配对同一 `state.sqlite3` 只打开一个 actor |

| `mutsuki-bot-service-host-integration` | 平台中立的显式 Host 装配面（Flow Router、Command、Interaction、Sandbox、Agent 链、恢复源、控制台桥）；不依赖任何聊天适配器或 B 站包，禁止再往里加业务 Runner |
| `mutsuki-bot-qq-service-host-integration` / `mutsuki-bot-bilibili-service-host-integration` | 各平台 owner 的 Host 装配（configured catalog、EventSource、健康映射、凭据存储）；产品合并 `configured_qq_plugin_catalog` / `configured_bilibili_plugin_catalog` |
| `mutsuki-bot-web-host-integration` | Bot 包提供的 WebHost 装配 helper，产品可选启用 |
| `mutsuki-bot-web-extension-platforms` | 通用平台控制台：按 `PlatformConsoleProvider`（`mutsuki-bot-management`）逐次解析在线适配器，新平台只需实现 provider，不新增页面 |

## 验证

Rust 改动运行 `cargo fmt --check`、`cargo check` 和 `cargo test`。平台和装配改动补充
外部边界 fake 或 smoke；最终报告实际命令、测试层级和统一 release revision。

提交前检查 `git status --short` 和定向 diff，提交标题使用中文短句。

## 大 blob 的 Host bridge（#182）

图片/卡片与 QQ 媒体消费方可以保留同步注入的 Host 资源 gateway。Host 桥接原生异步与 Offloaded
provider 时不占用 actor，但调用方 worker 仍会阻塞；异步代码必须改为 await 异步 gateway。不得改用
独立的 `LocalResourceClient`，也不得直接访问 provider 存储。见
../../docs/architecture/async-resource-creation.md；fake 测试与真实账号结果分开报告。

技能统一由本目录 `skills/` 路由；旧独立仓库的隐藏技能入口已合并，不维护第二套规则。
