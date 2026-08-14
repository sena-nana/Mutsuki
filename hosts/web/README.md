# MutsukiWebHost

Web application host at the same layer as `MutsukiTauriHost`.

It starts and hosts a `WebApplication` with HTTP/WebSocket, static assets,
typed management RPC/Event bridge, WebExtension loading, and a minimal Recovery Shell.

It does **not** ship Bot admin, database, log, metrics, or marketplace pages.

## Workspace

```text
crates/
  mutsuki-web-host/
  mutsuki-web-protocol/
  mutsuki-web-bridge/
  mutsuki-web-extension/
  mutsuki-web-recovery/
packages/
  @mutsuki/web-sdk/
  @mutsuki/web-shell/
  @mutsuki/web-build/
```

## Quick start (library)

```rust
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};

# async fn demo() -> Result<(), mutsuki_web_host::WebHostError> {
let mut host = MutsukiWebHost::builder()
    .application(MinimalWebApplication::empty("example.app"))
    .listen("127.0.0.1:0")
    .build()?;
host.start().await?;
host.stop().await?;
# Ok(())
# }
```

Default listen is loopback. Non-loopback requires TLS or explicit remote auth tokens.

## Frontend navigation

`@mutsuki/web-shell` renders extension pages from three scoped registries:

- `activities`: top/bottom work domains shown in the compact activity bar.
- `pages`: mountable page components with stable ids and Hash-route paths.
- `navigation`: links an `activityId` to a `pageId`; paths are never duplicated here.

The product WebApplication declares its fixed activity catalog. WebExtensions register only real
pages backed by their owner RPC, and the Shell removes entries unavailable to the authenticated
session. Every registration and mounted page must return a disposable lifecycle handle when it
owns subscriptions, timers, or other effects.

Published events remain in each session's bounded queue and wake that session's WebSocket writer
immediately. The readiness signal carries no payload or policy: `mutsuki-web-bridge` owns fanout and
budgets, while `mutsuki-web-host` only drains encoded events without an idle polling loop.

## Versions

- Web protocol: `1.0.0`
- Extension manifest: `1`

## Validate

```bash
cargo fmt --check
cargo check --locked
cargo test --locked
pnpm install --frozen-lockfile
pnpm typecheck
pnpm build
```

See `AGENTS.md` for ownership rules.
