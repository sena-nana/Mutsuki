# Web Console v2 迁移（前端平台轨道）

本文件记录 WebHost 前端平台（W0–W4、H）的破坏性变更与迁移步骤。Bot 领域后端的变更见
`bot-domain-v2.md`。

## W1 根 pnpm workspace

- `hosts/web/{package.json,pnpm-workspace.yaml,pnpm-lock.yaml}` 删除；前端命令改在仓库根执行：
  `pnpm install --frozen-lockfile && pnpm -r typecheck && pnpm -r test && pnpm -r build`（Node 26）。
- 新扩展前端放在 owner crate 的 `web/` 目录，依赖写 `catalog:`（共享运行时）与 `workspace:*`
  （`@mutsuki/*`）。测试使用 vitest。
- `@mutsuki/web-build` 改用 Vite：`buildExtension` 产出 `index.js` + 可选 `index.css` 与带
  `styles` 的 manifest；构建失败即拒绝（裸 import、打包 Vue、超预算、CSS hex 色值、`node:`）。

## W2 共享运行时

- 控制台文档内联 import map，`vue`、`vue-router`、`pinia`、`@mutsuki/web-sdk`、`@mutsuki/ui`、
  `@mutsuki/web-shell` 由 `shared/<name>.<hash>.js` 提供；扩展只写裸 import，不得打包这些依赖。
- `/mutsuki-ui.css`、`/shared/web-sdk.js`、`/shared/web-shell.js` 等固定路径不再提供；设计系统
  样式只以 `shared/ui.<hash>.css` 发布一次。各 crate 的 `assets/mutsuki-ui.css` 与
  `plugins/bot/scripts/sync-mutsuki-ui-css.sh` 已删除。
- `ExtensionManifest` 新增 `styles: Vec<String>`（缺省为空）。列出的样式必须是带哈希的
  asset，否则 `validate_manifest` 返回 `UndeclaredStyle`；`BundledManifest::build` 把 `.css`
  asset 自动列为样式。`console-options.json` 的每个扩展条目新增 `styles` URL 列表。
- `ConsoleAssetDirs` 不再暴露每页字段；用 `document_root()`、`shell_root()`、
  `page_assets(id)` 访问（id 与 `console-options.json` 一致，如 `qq-bot`、`bot-flow-editor`）。
- 控制台文档 CSP 的 `script-src` 为 `'self' 'sha256-<import map>'`，不再允许内联脚本。

## H Web 协议 1.1 与会话

- Web 协议升为 `1.1.0`（同 major 兼容）。`WireMessage::Error` 新增可选 `request_id`、
  `subscription_id`，旧帧缺省为 `None`；`code` 改为 `ProtocolError::code()` 的稳定值，
  原先所有失败共用的 `bridge_error` 不再出现。
- hello 凭据被拒返回 `auth_failed`（原 `bridge_error`），随后服务端关闭连接；客户端据此进入
  `unauthorized` 状态并停止重连。`ProtocolError` 新增 `AuthFailed` 变体。
- 浏览器登录改为 `POST /auth/session`（JSON `{"token": "…"}`）换取 HttpOnly 会话 cookie，
  `/ws` 握手携带 cookie 即可，hello 不再需要 token；`POST /auth/logout` 注销。控制台刷新不再
  要求重新输入口令。
- `/ws`、`/auth/*` 拒绝与请求 Host 不一致的 `Origin`（403）。自带 Origin 的跨站嵌入需与 Host
  同源。

## W3 SDK v2

- 错误统一为 `RpcError { code, message, details, retryable }`；`WebBridgeError` 是其子类，
  仍可 `instanceof`。`BridgeErrorCode` 移到 `errors.ts` 并新增 `auth_failed`。
- 默认超时改为读 15s、写 30s（原统一 30s）；`requestTimeoutMs` 仍同时覆盖两者，新增
  `readTimeoutMs`/`writeTimeoutMs`。
- `EventClient.subscribe` 第四个参数 `onError` 接收订阅被拒；被拒订阅不再在重连时恢复。
- 新增 `defineRpc`/`query`/`mutation`/`topic`、`useRpc`/`useMutation`/`useSubscription`/
  `usePolling`/`useSession`/`useConnectionState`、`createI18n`/`defineMessages`/`useI18n`、
  格式化函数、`defineExtension` 与 `MAX_EXTENSIONS`（Shell 超出即记失败）。测试替身在
  `@mutsuki/web-sdk/testing`（不进入共享运行时）。

## W4 设计系统

- `@mutsuki/ui` 改为 Vue 组件库：`ConsoleShell`、`createConsoleShellElement`、`ConsoleNavItem`
  与 `consoleShell.ts` 删除（`@mutsuki/web-shell` 不再转出它们）；新 Shell 在 W5 用 `Mk*` 组件
  组装。
- 新增 `--mk-*` 产品 token（`tokens.css`）与 `Mk*` 组件；组件样式进入同一份
  `shared/ui.<hash>.css`。确认/输入/提示改用 `useConfirm`/`usePrompt`/`useToast`
  （需 `installMutsukiUi(app)` 与一个 `<MkOverlayHost />`）。
- `I18n` 新增 `has(key)`；`@mutsuki/web-shell` 的类型检查与构建改用 `vue-tsc`。

## W5 Shell v2

- 控制台入口由 `@mutsuki/bot-console`（`mutsuki-bot-web-host-integration/web`，产物
  `web/browser/console.js`）提供，`assets/console-bootstrap.js` 删除。它读取
  `console-options.json` 并调用 `@mutsuki/web-shell` 的 `startConsole`。
- `console-options.json` 改为 `{ brand, homePageId, layout: { activities, placements }, extensions }`；
  顶层 `activities` 不再存在。activity 只由产品布局定义：`WebConsoleConfig.layout`
  （`ConsoleLayout`，缺省为 概览/插件/诊断/设置），Bot 产品用 `console_layout()`（总览、平台与消息、
  会话与 Agent、自动化、沙盒、数据、插件、系统、设置）。构建控制台时校验布局（重复 id、缺少
  `settings`、悬空 placement 均拒绝）。
- 扩展调用 `ctx.activities.register` 不再生效；未知 activity 记入失败面板。旧导航按
  `placements`（导航 id 或旧 activity id → 布局 activity）落位，随各扩展迁移逐条删除。
- 外观设置固定在 `settings` activity 的 `webui.settings` 页（不再随 `plugins` 切换）；新增语言、
  退出登录与关于。`mountWebShell`、`configureActivities`、`configureWebUiSettings` 删除，改用
  `startConsole`/`configureLayout`。`PageRegistration.component` 可为 Vue 组件，旧 `{ mount }`
  经 `legacyMount` 适配。
- `@mutsuki/web-shell` 改由 Vite 构建（SFC），共享运行时从其 `dist` 打包。
- 新增 `--mk-color-accent-solid`/`-hover`：浅色主题下把强调色压暗用于实心控件（主按钮、跳转链接、
  登录按钮），使白字对比度 ≥ 4.5:1。
- Playwright smoke（`pnpm --filter @mutsuki/bot-console e2e`，CI `console-e2e`）基于
  `examples/embedded_console_fixture.rs`；该示例读取 `MUTSUKI_CONSOLE_FIXTURE_LISTEN`，并启用
  config（demo provider）与沙盒页。

## W6 Host 侧扩展 Vue 化

- `upgrade`、`overview`、`control` 三个扩展改为各自 crate 下 `web/` 的 Vue 包
  （`@mutsuki/web-ext-{upgrade,overview,diagnostics}`），产物 `web/browser/index.js` 随源码提交并由
  crate `include_str!`；`assets/index.js` 删除，手写资产白名单相应缩短。
- 扩展 id 改为 `mutsuki.web.upgrade`、`mutsuki.web.overview`、`mutsuki.web.diagnostics`（原
  `control`）。`WebExtension` 新增 `namespace()`（缺省为 id）：三者的 RPC 命名空间保持
  `upgrade`/`overview`/`control`，`control.changed` 话题不变。同一命名空间只允许一个扩展。
  `ExtensionRecord` 新增 `namespace` 字段。
- 控制台 `console-options.json` 与 `ConsoleAssetDirs::page_assets` 使用新 id；挂载目录为
  `extensions/{overview,diagnostics,upgrade}`。页面 id（`overview.page`、`control.*`、
  `upgrade.page`）不变，已保存的路由继续有效。
- RuntimeEvent 时间线投影迁入 diagnostics 包（`runtime-events.ts` + `RuntimeTimeline.vue`）；
  控制台不再把 `trajectory-model.js`/`trajectory-view.js` 写到文档根或 control 目录，
  `mutsuki_agent_web_extension::materialize_trajectory_assets` 改为 crate 私有（Agent 页仍自带）。
- 确认/提示改用 `useConfirm`（排空、关闭服务需输入确认词），失败以 toast 呈现；不再使用原生
  `confirm()`/`prompt()`。`@mutsuki/web-build` 新增 `--no-manifest`；SDK 新增 `useMessages`；
  `MkDataTable` 的行类型放宽为任意对象（`rowKey: string`）。

## W7 平台与数据页

- 数据库页改为 `@mutsuki/bot-ext-database`（Vue），扩展 id `mutsuki.bot.database`，命名空间仍为
  `database`；导航进入 `data` activity（产品布局删除 `database.nav` placement，通用缺省布局新增
  `data`）。原 cargo 测试里 spawn `node` 校验单元格转义的用例与 `assets/escape.test.mjs` 删除，
  改由 vitest 断言单元格以文本渲染、不产生元素。
- 新增通用平台账号页 `@mutsuki/bot-ext-platforms`（`mutsuki.bot.platforms`，`platforms` activity）：
  按 `PlatformAdapterView` 渲染账号字段（按 `format`）、操作（`danger` 需确认，按 capability
  禁用）与 `platform.detail:<platform>` 面板（`ExtensionSlots` 新增该模式，面板以
  `{ adapter, account }` 为 props）。设置 `BotConsoleServices.platforms` 时控制台挂载该页。
- QQ 扩展改为 `@mutsuki/bot-ext-qq`，id `mutsuki.bot.qq`（命名空间仍为 `qq-bot`）：只保留概览卡片，
  新增「投递记录」「交互等待」两页（仍走 `qq-bot.deliveries.list|interactions.list|write`，计划 W7a
  回退方案）；账号管理迁到平台账号页。QQ 管理错误改为 `ExtensionError::Rpc { code, message }`，
  客户端按 `qq.owner_unavailable` 等 code 分支，不再解析 JSON 文本。
- `@mutsuki/web-sdk/testing` 的 `FakeHandler` 返回值放宽为 `unknown`。
