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
