---
name: integration-testing
description: Validate the first-party Mutsuki Bot product through real ServiceRuntime assembly, external-boundary fakes or smoke accounts, health, dependency portability, and graceful shutdown. Use for product E2E, fake server E2E, startup failures, reconnection, or cleanup tests.
---

# Integration Testing

验证用户获得的产品路径，不用裸 CoreRuntime 结果代替产品验收。

## 层级

- 单元测试可直接覆盖配置、纯函数、Runner entry 和 manifest builder。
- 集成测试可替换 transport，但必须保留正常 task、Runner 和 ResultRouter 路径。
- 产品 E2E 从外部配置启动真实 `ServiceRuntime`；fake server 只替换 QQ/HTTP/WebSocket/Provider 等外部系统，业务处理器仅可作为 test-only owner double。
- 在测试临时目录生成所有配置，不提交可直接运行的 TOML/JSON。
- 控制台组件层：每个扩展的 `web/` 包用 vitest + fake bridge（`@mutsuki/web-sdk/testing`）断言调用、
  状态与错误呈现（`pnpm -r test`）；不启动 Host，不替代下面的浏览器层。
- 控制台浏览器层：`plugins/bot/crates/mutsuki-bot-web-host-integration/web` 的 Playwright smoke
  （`pnpm --filter @mutsuki/bot-console e2e`，CI job `console-e2e`，必需检查）对
  `examples/embedded_console_fixture.rs` 运行。fixture 装配 demo `ConfigService`、fake QQ 账号与
  平台 provider、无 Flow 的 `SandboxService`，布局含 `platforms`/`sandbox`。覆盖：口令登录与拒绝、
  落地页与未知路由回退、每页挂载无 boundary/console 错误、平台账号来自适配器描述符、配置 hub
  的 SchemaForm 校验→应用并在刷新后保留、沙盒写入被 owner 拒绝时显示其消息并保留草稿、主题切换、
  浅/深色下各 Vue 页面 axe 无 serious/critical、刷新后 cookie 会话恢复。本地默认复用已在运行的
  fixture，验收用 `CI=1` 强制重新构建。它是 fake 层级，不替代真实账号 smoke。
- 真实 smoke 默认使用仓库内被忽略的本地产品配置和专用 secret 文件，也可由环境覆盖路径或 secret；不提交凭据或敏感输出。
- B 站真实 smoke（`tests/bilibili_real_smoke.rs`，ignored）在临时实例中经控制台扫码登录：
  `MUTSUKI_BILIBILI_QR_PNG` 指定二维码输出，需要有人扫码；过期自动换码至多 3 次。它断言 Cookie
  经 HTTP effect 写入 Host secret、预览走鉴权请求，且 Cookie 不出现在进程输出与日志中。读回 secret
  的失败信息不得回显 secret 文件内容。

## 验收

1. 验证零插件 Runtime，以及配置选择的平台 transport、EventSource 和 health；业务闭环由对应 owner 仓库验收。
2. 验证 transport/EventSource、重连或恢复，以及 stop 后 socket、worker、IPC 和后台任务释放。
3. 验证缺失配置、secret、capability、artifact 和 deployment 在启动阶段失败。
4. 验证 health/控制面真实，日志和 trace 不泄露 secret。
5. 在无兄弟仓库的主仓 clone 中验证统一 release、构建、启动和测试。

断言协议流和外部行为；报告实际测试层级，未执行真实 smoke 时不得用 mock 代替。
