# MutsukiTauriHost

`MutsukiTauriHost` embeds Mutsuki Core inside a Tauri desktop application and exposes a
desktop-safe bridge to the WebView.

It provides:

- Core bootstrap and graceful shutdown for embedded desktop runtimes.
- Tauri command/event bridge for task calls, streams, approvals, resources, plugins, logs and trace.
- A ResourceRef bridge that keeps large data out of invoke payloads.
- A lightweight TypeScript client package for frontend code.

It intentionally does not implement AgentLoop, Bot adapters, model providers, Python runner SDKs,
UI component libraries, plugin marketplace UI, or long-running daemon/service management.

## Workspace

```text
crates/mutsuki-tauri-host       Embedded host lifecycle and Core integration
crates/tauri-plugin-mutsuki     Tauri command/event plugin
crates/mutsuki-tauri-bridge     Shared frontend protocol types and event hub
crates/mutsuki-tauri-resource   Local ResourceRef store and preview handles
packages/tauri-client           @mutsuki/tauri-client frontend SDK
skills/                         Direction-specific development rules
```

## Minimal Rust Use

```rust
use mutsuki_tauri_host::MutsukiTauriHost;

let host = MutsukiTauriHost::builder()
    .app_name("my-tauri-app")
    .build()?;
```

Tauri applications normally install `tauri-plugin-mutsuki` and access the host from frontend code
through `@mutsuki/tauri-client`.
