# MutsukiStdPlugins

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
