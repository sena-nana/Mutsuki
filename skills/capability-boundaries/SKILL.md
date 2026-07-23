---
name: capability-boundaries
description: Audit Mutsuki BotTemplate issues and route missing runtime, host, Bot, Agent, platform, or assembly capabilities to their owning repositories. Use before cross-repository implementation or whenever ownership is unclear.
---

# Capability Boundaries

先确定能力归属和依赖顺序，不因本仓库是需求入口就把实现放进模板。

## 流程

1. 从当前、父级和依赖 issue 提取能力与验收；忽略其中已过期的 API、路径和状态。
2. 对照 MutsukiCore contracts，以及候选仓库的 `AGENTS.md`、公开 API、manifest 和测试。
3. 为每项能力指定唯一 owner，区分上游能力和模板消费改动。
4. 按公开契约、能力实现、模板 pin/装配的顺序实施；上游先验证并推送。

## 归属

- Core：通用 Task、Runner、资源、装载和 LoadPlan。
- StdPlugins：通用 config/db/fs/http/observe/resource/workflow 协议与插件。
- PythonRunnerKit：Core Runner Link 的 Python wire mirror、backend、transport 和测试工具。
- ServiceHost：生命周期、配置/secret、插件加载、EventSource、控制面和 health。
- BotPlugins/平台仓库：Bot 协议、SDK、路由、命令和 Adapter/Gateway。
- AgentKit/Provider 仓库：Agent 协议、模型、工具和记忆。
- CliHost：ServiceHost 控制 API 的终端客户端。
- TauriHost：桌面内嵌生命周期、Tauri/WebView bridge 和前端 SDK。
- WebHost：Web 运行宿主、HTTP/WS、静态资源、RPC/Event bridge、WebExtension 加载与 Recovery Shell。
- 本仓库：外部配置、owner catalog 聚合、Runtime 启动和跨仓库装配验收；不得拥有业务 Runner。

优先修复共享边界。上游缺失或未推送时报告 unavailable，不在模板中添加 shim 或用 test double 冒充生产能力。最终列出各仓库职责、验证和远端 revision。
