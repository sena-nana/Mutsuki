# MutsukiWebHost 工作规范

本仓库是 **与 MutsukiTauriHost 同层的 Web 运行宿主**。它负责启动并承载一个
`WebApplication`：HTTP/WebSocket、静态资源、RPC/Event bridge、WebExtension 加载、
会话/认证/capability，以及最小 Recovery Shell。

它不是固定 Bot 管理后台，也不实现数据库/日志/指标/市场页面，更不向插件暴露
Axum/HTTP internals 作为稳定 ABI。

## 阅读顺序

1. 当前及关联 issue，确认目标、依赖和验收场景。
2. `../MutsukiCore/AGENTS.md`、`../MutsukiLink/AGENTS.md`（若改动 Link/standalone）。
3. 本文件路由的相关技能，再检查当前实现、远端 commit 和 lockfile。

Issue 是需求线索，不是当前 API 的事实源。存在 `.codegraph/` 时，定位代码先用 CodeGraph。

## 技能路由

- `skills/capability-boundaries/SKILL.md`：能力归属与跨仓库顺序。
- `skills/web-host/SKILL.md`：Host 生命周期、bridge、extension、recovery、安全与测试。
- `skills/web-build/SKILL.md`：前端 SDK/Shell 与预编译构建工具。

职责不明先读 capability-boundaries。

## 职责边界

| 组件 | 职责 |
| --- | --- |
| `mutsuki-web-host` | Host 生命周期、监听、应用装配、静态托管 |
| `mutsuki-web-protocol` | 前后端 descriptor、RPC/Event 协议与版本 |
| `mutsuki-web-bridge` | WebSocket/HTTP ↔ typed management protocol |
| `mutsuki-web-extension` | Extension manifest、registry、资源校验 |
| `mutsuki-web-recovery` | 最小恢复与安全模式 Shell |
| `@mutsuki/web-sdk` | Vue/WebExtension 前端 SDK |
| `@mutsuki/web-shell` | 可复用 Shell 基础实现 |
| `@mutsuki/web-build` | Vue/TS/CSS 标准构建工具（运行时不调用） |

跨仓库：`MutsukiCore`（仅通用协议缺口）、`MutsukiLink`（standalone 桥接）、
`MutsukiServiceHost`/Bot（嵌入宿主）、`MutsukiBotPlugins`（Schema-first；默认 Web
配置插件不在本仓库）、`MutsukiTauriHost`（同层对齐，不复制实现）。

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
10. 禁止仓库外 Cargo `path`/本地 `[patch]`；跨仓库 Git 依赖固定 `rev`。
11. 不向插件暴露 Axum/Hyper 类型作为稳定 ABI。
12. 不在本仓库实现数据库、日志、指标、市场或 Bot 管理业务页面。

## 验证

```bash
cargo fmt --check
cargo check --locked
cargo test --locked
cargo metadata --locked --format-version 1
pnpm install --frozen-lockfile
pnpm typecheck
pnpm build
```

最终说明必须列出实际命令和结果；测试断言行为，不只匹配日志或文案。
