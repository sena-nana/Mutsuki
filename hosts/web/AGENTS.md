# MutsukiWebHost 工作规范

本目录是 **与 MutsukiTauriHost 同层的 Web 运行宿主**。它负责启动并承载一个
`WebApplication`：HTTP/WebSocket、静态资源、RPC/Event bridge、WebExtension 加载、
会话/认证/capability，以及最小 Recovery Shell。

它不是固定 Bot 管理后台，也不实现数据库/日志/指标/市场页面，更不向插件暴露
Axum/HTTP internals 作为稳定 ABI。

## 阅读顺序

1. 当前及关联 issue，确认目标、依赖和验收场景。
2. `../../AGENTS.md`、`../../crates/link/AGENTS.md`（若改动 Link/standalone）。
3. 本文件路由的相关技能，再检查当前实现、远端 commit 和 lockfile。

Issue 是需求线索，不是当前 API 的事实源。

## 技能路由

- `skills/capability-boundaries/SKILL.md`：能力归属与跨仓库顺序。
- `skills/web-host/SKILL.md`：Host 生命周期、bridge、extension、recovery、安全与测试。
- `skills/web-build/SKILL.md`：前端 SDK/Shell 与预编译构建工具。
- `skills/web-extension-authoring/SKILL.md`：扩展前端（`web/` 包）的契约、composables、i18n、预算与测试。

职责不明先读 capability-boundaries。

## 职责边界

| 组件 | 职责 |
| --- | --- |
| `mutsuki-web-host` | Host 生命周期、监听、应用装配、静态托管 |
| `mutsuki-web-protocol` | 前后端 descriptor、RPC/Event 协议与版本 |
| `mutsuki-web-bridge` | WebSocket/HTTP ↔ typed management protocol |
| `mutsuki-web-extension-api` | 插件侧 WebExtension trait、RPC registry、bundled manifest |
| `mutsuki-web-extension` | Host registry、budget、reactor 分发 |
| `mutsuki-web-recovery` | 最小恢复与安全模式 Shell |
| `mutsuki-web-shared-runtime` | 嵌入 `@mutsuki/web-runtime` 预构建的共享模块：内容寻址文件名、import map、CSP 哈希 |
| `@mutsuki/web-sdk` | Vue/WebExtension 前端 SDK：bridge、`RpcError`、`defineRpc`、composables、i18n、`defineExtension`；`/testing` 提供 fake bridge |
| `@mutsuki/web-shell` | 可复用 Shell 基础实现 |
| `@mutsuki/ui` | LiliaUI token/layout 包装（styles + ConsoleShell）；视觉基座来自 pinned `@lilia/theme`，构建期由 `bundle-css.mjs` 打进 `dist/mutsuki-ui.css`，经共享运行时以 `shared/ui.<hash>.css` 发布一次 |
| `@mutsuki/web-runtime` | 构建 `browser/shared/{vue,vue-router,pinia,web-sdk,ui,web-shell}.js` 与 `ui.css`（提交产物，Rust 嵌入） |
| `@mutsuki/web-build` | Vite + `@vitejs/plugin-vue` 扩展构建与产物门禁（运行时不调用） |

跨 package：Core（仅通用协议缺口）、Link（standalone 桥接）、ServiceHost/Bot
（嵌入宿主）、BotPlugins（Schema-first；默认 Web 配置插件不在本目录）、TauriHost
（同层对齐，不复制实现）。

## Hard Rules

1. WebHost 只提供运行环境、桥接和扩展加载基础设施；具体页面由 WebApplication / WebExtension 实现。
2. 前端只能通过受控 RPC/Event API 访问后端；扩展无法获得原始 token、Host IPC 或插件进程 handle。
3. capability 必须在服务端强制检查；禁止字符串前缀决定错误/恢复策略。
4. Vue SFC/TS/CSS 必须在发布期预编译为 ESM；生产 WebHost 禁止依赖 Node/Vite 或运行时编译 `.vue`。
5. shared runtime（vue / vue-router / pinia / `@mutsuki/web-sdk` / `@mutsuki/ui`）由 Shell 提供，插件不得重复打包。
6. 扩展通过稳定 registry 与明确扩展点注册；不允许无限制全局 Vue 注册接口。
7. 扩展失败由 Error Boundary / Recovery Shell 隔离，单个扩展不得拖垮整个 Shell。
8. 所有连接、队列、payload、静态缓存均有明确预算；缺失预算必须结构化失败。
9. 本机默认只监听 loopback；非 loopback 必须有明确 TLS/远程认证策略。
10. 仓内 Mutsuki 依赖必须继承根 Workspace 的 path；禁止内部 Git pin、仓库外 Cargo `path` 和本地 `[patch]`。
11. 不向插件暴露 Axum/Hyper 类型作为稳定 ABI。
12. 不在本目录实现数据库、日志、指标、市场或 Bot 管理业务页面。

## 验证

```bash
cargo fmt --check
cargo check --locked
cargo test --locked
cargo metadata --locked --format-version 1
# 前端包属于仓库根 pnpm workspace，在仓库根执行（Node 26）
pnpm install --frozen-lockfile
pnpm -r typecheck
pnpm -r test
pnpm -r build
```

`browser/*.js` 等被 Rust `include_bytes!` 的产物随源码提交；构建后 `git status --porcelain`
必须为空。

最终说明必须列出实际命令和结果；测试断言行为，不只匹配日志或文案。
