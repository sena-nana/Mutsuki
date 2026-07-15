# Mutsuki Repository Boundaries

依赖方向固定为：Core → 标准/领域能力 → Host integration → 产品模板。下游不得复制上游
实现或通过本地路径反向引用。

| Owner | Owned implementation | Allowed consumers |
| --- | --- | --- |
| MutsukiCore | contracts、TaskPool、Runner、Resource facts、LoadPlan、通用 Host/Runner helper | 全部 runtime 消费方 |
| MutsukiStdPlugins | config/db/fs/http/observe/resource/workflow 协议与插件 | Host 与产品 |
| MutsukiPythonRunnerKit | Python contract mirror、Runner backend、transport；fake 仅在 testing | Python 插件进程 |
| MutsukiServiceHost | 服务生命周期、配置/secret、EventSource、控制面、监督策略 | CLI、integration、产品 |
| MutsukiBotPlugins | Bot 协议/Runner/平台 Adapter；ServiceHost bridge 位于独立 integration crate | Bot 产品 |
| MutsukiAgentKit | AgentLoop、tool、memory、model；网络 Provider 仅走 effect Runner | Agent 产品与模板测试 |
| MutsukiCliHost | 公开 ControlClient 的终端 UI | 用户 |
| MutsukiTauriHost | 桌面生命周期、Tauri/WebView bridge、桌面策略 | 桌面产品 |
| MutsukiDistributedHost | 外置控制/数据面、远程执行、HA、恢复、调度、信任与审计 | 部署系统与产品装配层 |
| MutsukiBotTemplate | 外部配置入口、owner catalog 聚合、Runtime 启动和跨仓库装配验收 | 产品 fork |

链接进产品的原生实现只能以 owner 提供的 `ConfiguredPluginFactory` 进入模板。模板注册可用
catalog，外部 `[[plugins.configured]]` 决定实际启用项；owner 配置对模板保持不透明，并在
RuntimeProfile/LoadPlan 冻结前完成校验与安装。

模板不得注册自有业务 manifest 或 Runner。命令、回复、Agent 流程及其他产品行为必须由
BotPlugins、AgentKit 或独立业务仓库提供；零插件模板只启动空闲 Runtime。

所有跨仓库 Cargo 依赖使用远端 Git URL 和已推送固定 `rev`。配置与 secret 不提交；缺失
capability、artifact、backend 或 secret 必须 fail loud。

分布式配置只选择外置 `MutsukiDistributedHost` 部署和任务策略。模板链接其 contracts 以校验
mode，并校验外部 revision、拓扑、通道、预算和策略文件；任务语义由 DistributedHost 拥有。
模板不把 DistributedHost 实现链接进 ServiceRuntime，不监管
Sidecar，也不向插件注入 ClusterContext、NodeId 或 TrustLevel。控制与大数据通道必须分离，
Leader 仅处理有界控制描述符。
