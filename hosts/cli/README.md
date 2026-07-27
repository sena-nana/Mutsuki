# MutsukiCliHost

Mutsuki 的独立命令行与终端 UI Host。它连接正在运行的 `MutsukiServiceHost`，通过公开、
带鉴权的控制 API 展示 service/core/plugin/runner/event-source 健康状态和实时日志。

本仓库接替了曾内建在 `MutsukiServiceHost` 中的终端 UI。它不注册 `terminal.tui` 插件，
不提供会话模拟，也不在控制面之外实现业务 runtime 路径。

## 运行

先启动 ServiceHost，然后运行：

```powershell
cargo run -- --home ../MutsukiServiceHost/.mutsuki-dev --token dev-token
```

- `R`：立即刷新状态、健康和日志
- `Esc` / `Q` / `Ctrl+C`：退出

CLI 不加载 ServiceHost profile 或配置文件。也可以直接指定公开控制 endpoint：

```powershell
cargo run -- --transport named-pipe --endpoint mutsuki --token dev-token
```
