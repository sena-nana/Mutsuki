# MutsukiTauriHost

`MutsukiTauriHost` embeds Mutsuki Core inside a Tauri desktop application and exposes a
desktop-safe bridge to the WebView.

It provides:

- Core bootstrap and graceful shutdown for embedded desktop runtimes.
- Tauri command/event bridge for task calls, streams, approvals, resources, plugins, logs and trace.
- A ResourceRef bridge that keeps large data out of invoke payloads.
- Cross-app delivery (`request_app` / `deliver_to_app`): discover/activate a peer Tauri app,
  wait until its MutsukiLink endpoint and typed capability are ready, then transmit a request and
  receive an idempotent receipt over local IPC (Named Pipe / Unix Domain Socket).
- A lightweight TypeScript client package for frontend code.

It intentionally does not implement AgentLoop, Bot adapters, model providers, Python runner SDKs,
UI component libraries, plugin marketplace UI, or long-running daemon/service management.
Ordinary app-to-app calls do not require `MutsukiServiceHost`; a long-lived daemon remains optional
for work that must continue after every UI app has quit.

## Workspace

```text
crates/mutsuki-tauri-host       Embedded host lifecycle, Core integration, app delivery
crates/tauri-plugin-mutsuki     Tauri command/event plugin
crates/mutsuki-tauri-bridge     Shared frontend protocol types and event hub
crates/mutsuki-tauri-resource   Local ResourceRef store and preview handles
packages/tauri-client           @mutsuki/tauri-client frontend SDK
skills/                         Direction-specific development rules
docs/examples/request-app.md    Minimal cross-app delivery example
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

Cross-app delivery is exposed as `AppDeliveryService::request_app`. Business code must not launch
peers, poll sockets, or stuff task payloads into process arguments; see
`docs/examples/request-app.md`.

Task completion is driven by HostRuntime terminal notifications. One batch query refreshes all
tracked handles after a notification; idle or Waiting tasks do not start a periodic poller. Runtime
events and traces are read with bounded cursor pages, task result event retention has per-task and
global limits, and frontend observation events are emitted in bounded batches.

Large resources use bounded 64 KiB invoke chunks. Preview/object URLs are backed by the registered
`mutsuki-resource` protocol and can be explicitly released; a 1 MiB or 64 MiB body is never embedded
in command/event JSON.

The unified benchmark covers 1, 100 and 1000 active Waiting tasks, executable command/event
serialization, bounded frontend retention and 1 MiB/64 MiB ResourceRef paths:

```text
python3 crates/mutsuki-tauri-benchmarks/scripts/run-reference.py \
  --mode reference --output target/mutsuki-benchmarks/tauri-reference.json
```

The bridge fixture includes the embedded Rust Host and the JSON serialization/bookkeeping used by
the Tauri adapter. It does not instantiate a real OS WebView, so its results are not presented as a
full WebView roundtrip. Fixed reference-machine runs may add that outer boundary without mixing it
with UI rendering performance. See `docs/performance-model-v1.md`.
