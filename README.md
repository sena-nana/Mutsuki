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

Task completion is driven by HostRuntime terminal notifications. One batch query refreshes all
tracked handles after a notification; idle or Waiting tasks do not start a periodic poller. Runtime
events and traces are read with bounded cursor pages, task result event retention has per-task and
global limits, and frontend observation events are emitted in bounded batches.

The task-pump benchmark covers 1, 100 and 1000 active Waiting tasks and records process CPU time,
completion latency and actor command counts:

```text
cargo run --release -p mutsuki-tauri-host --example task_pump_benchmark -- artifacts/perf/issue2-task-pump.json
```
