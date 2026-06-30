# MutsukiTauriHost 工作规范

本仓库是 **MutsukiCore 在 Tauri 桌面应用中的内嵌 Host**。它负责 Core lifecycle、Tauri command/event bridge、ResourceRef WebView bridge、approval UI bridge、插件/runner 桌面加载、桌面 HostServices、日志和 trace 桥接。

它不是 ServiceHost、AgentHost、BotHost、模型 Provider、Python SDK、LiliaUI 组件库或业务插件集合。

## 阅读顺序

按改动方向选择对应 skill，并先读完该 skill 的 `SKILL.md`：

1. `skills/runtime/SKILL.md`：Core bootstrap、Host lifecycle、shutdown/drain、桌面 HostServices。
2. `skills/tauri-bridge/SKILL.md`：Tauri commands、events、task invoke、stream bridge、状态管理。
3. `skills/resource-bridge/SKILL.md`：ResourceRef、文件导入、分块读取、临时预览 URL、WebView 安全边界。
4. `skills/approval-security/SKILL.md`：审批请求、approval token、frontend session、权限边界和审计。
5. `skills/plugin-runner/SKILL.md`：builtin/plugin manifest、dev reload、external runner、runner 环境隔离。
6. `skills/frontend-sdk/SKILL.md`：`@mutsuki/tauri-client` 的 TS API、异步事件、资源和审批封装。
7. `skills/observe/SKILL.md`：日志、trace、health、panic/crash、runner stdout/stderr 事件桥。

## Hard Rules

- TauriHost 管桌面应用和 WebView 桥接；Core 管任务系统；插件管领域能力；产品仓库管业务逻辑。
- 不在 Host 中实现 AgentLoop、ToolRouter、BotEventRouter、模型 Provider、Python runner SDK 或 LiliaCode 专属逻辑。
- 前端只能通过协议和 capability 调用 Core，不能直接获得 Core handle 或真实文件/进程/secret handle。
- ResourceRef 是跨边界 descriptor；大文本、图片、音视频、模型文件不得 base64 塞进 Tauri invoke payload。
- 审批、插件、runner、资源和 dev command 必须真实接入后端；禁止 UI 或 API 看起来可用但未接线。
- 修复问题必须定位根因并在正确层级修正，禁止绕症状打补丁。
- 新测试必须验证功能行为；无功能变动不添加低价值测试，不硬匹配日志或字符串。

## CodeGraph

如果仓库根目录存在 `.codegraph/`，需要理解或定位代码时先使用 CodeGraph，再使用 `rg` 或直接读文件。没有 `.codegraph/` 时跳过。

## 验证

Rust 改动至少运行：

```powershell
cargo fmt --check
cargo check
```

TypeScript client 改动至少运行：

```powershell
pnpm --filter @mutsuki/tauri-client typecheck
```

最终说明必须列出实际执行过的验证命令与结果。
