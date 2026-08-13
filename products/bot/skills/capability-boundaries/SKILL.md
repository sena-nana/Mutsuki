---
name: capability-boundaries
description: Audit Mutsuki Bot product work and route missing runtime, host, Bot, Agent, platform, or assembly capabilities to their owning packages. Use before cross-package implementation or whenever ownership is unclear.
---

# Capability Boundaries

先确定能力归属和依赖顺序，不因产品目录是需求入口就把 owner 实现放进产品装配。

## 流程

1. 从当前、父级和依赖 issue 提取能力与验收；忽略其中已过期的 API、路径和状态。
2. 对照根 contracts，以及候选 package 的 scoped `AGENTS.md`、公开 API、manifest 和测试。
3. 为每项能力指定唯一 owner，区分 owner 能力和产品装配改动。
4. 按公开契约、能力实现、产品装配的顺序实施，并在同一主仓 revision 原子验证。

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
- 本目录：第一方 Bot 外部配置、owner catalog 聚合、Runtime 启动和跨 package 产品验收；不得拥有业务 Runner。

优先修复共享边界。owner package 缺失时报告 unavailable，不在产品中添加 shim 或用 test double 冒充生产能力。最终列出各 package 职责、验证和统一 release revision。
