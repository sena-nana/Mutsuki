---
name: integration-testing
description: Validate Mutsuki BotTemplate through real ServiceRuntime assembly, external-boundary fakes or smoke accounts, health, dependency portability, and graceful shutdown. Use for mock E2E, fake server E2E, startup failures, reconnection, or cleanup tests.
---

# Integration Testing

验证用户获得的产品路径，不用裸 CoreRuntime 结果代替产品验收。

## 层级

- 单元测试可直接覆盖配置、纯函数、Runner entry 和 manifest builder。
- 集成测试可替换 transport，但必须保留正常 task、Runner 和 ResultRouter 路径。
- 产品 E2E 从外部配置启动真实 `ServiceRuntime`；fake server 只替换 QQ/HTTP/WebSocket/Provider 等外部系统，业务处理器仅可作为 test-only owner double。
- 在测试临时目录生成所有配置，不提交可直接运行的 TOML/JSON。
- 真实 smoke 默认使用仓库内被忽略的本地产品配置和专用 secret 文件，也可由环境覆盖路径或 secret；不提交凭据或敏感输出。

## 验收

1. 验证零插件 Runtime，以及配置选择的平台 transport、EventSource 和 health；业务闭环由对应 owner 仓库验收。
2. 验证 transport/EventSource、重连或恢复，以及 stop 后 socket、worker、IPC 和后台任务释放。
3. 验证缺失配置、secret、capability、artifact 和 deployment 在启动阶段失败。
4. 验证 health/控制面真实，日志和 trace 不泄露 secret。
5. 在独立 checkout 验证远端依赖、构建和测试。

断言协议流和外部行为；报告实际测试层级，未执行真实 smoke 时不得用 mock 代替。
