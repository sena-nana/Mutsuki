# ServiceHost 示例运行说明

该示例通过 Mutsuki SDK 的 `HostRuntime` 接口运行 QQBot Echo 模拟链路：

```powershell
cargo run -p service-host-example
```

示例会注册 QQBot Adapter、事件路由、命令解析器和 Echo 业务 Runner，并让 Runtime 按标准任务链路运行到空闲；
随后通过注入到 `BotContext` 的 Host `TaskSubmitter` 再发送一条 Bot 消息，并通过同一 SDK 边界查询任务结果。

该命令使用 recording client，不会向 QQBot 发起真实网络请求。输出内容是记录到的 OpenAPI 请求，
用于验证消息是否经过标准 `mutsuki.bot.*` 任务链路。

## 配置边界

本仓库是插件层，不负责定义 Host 的配置文件格式、配置路径、目录创建或环境变量加载规则。
生产 Host 应使用 `MutsukiServiceHost` 已有的配置能力构造 `ServiceConfig` 和 `QqBotConfig`，
再通过 `QqBotPluginBundle::install` 注册 Adapter。Client Secret 由 ServiceHost 的
`HostEventSourceConfig::secret` 边界注入，不进入普通配置对象、任务载荷或追踪元数据。

真实账号的启动、健康检查和停止命令应由具体产品 Host 提供；插件层只提供 Adapter、Runner、
EventSource 和 ServiceHost 装配接口。
