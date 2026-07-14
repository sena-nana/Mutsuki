# MutsukiStdPlugins

标准插件 owner catalog 由 `mutsuki-std-plugins::configured_std_plugin_catalog()`
提供。目前 catalog 注册内存资源 Provider 与显式配置的 Chromium browser 插件。

Browser 协议为 `mutsuki.browser.snapshot`。调用方必须预创建 schema 为
`mutsuki.browser.snapshot.output.v1` 的 COW output resource；runner 完成后写入
`{ final_url, title, html }` JSON，调用方再打开最新 descriptor。Chromium 配置必须
显式提供本机 executable、domain allowlist、timeout 与 DOM byte 上限；插件不会自动
下载浏览器，也不提供任意 JavaScript 执行面。

Mutsuki 的领域中立标准协议与插件集合。Core 只提供 runtime 机制；本仓库提供
config、database、filesystem、HTTP、observe、resource 和 workflow 能力。

## Workspace

- `protocols/`：纯协议 ID、schema 与 wire surface。
- `plugins/`：batch-first Runner、provider 与 effect gateway。

所有 Core crate 均来自固定远端 revision，仓库可脱离兄弟目录独立构建。

## Verification

```powershell
cargo fmt --check
cargo check
cargo test
```
