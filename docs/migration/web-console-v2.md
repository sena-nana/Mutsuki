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
