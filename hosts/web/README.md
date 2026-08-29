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

`@mutsuki/web-shell` renders extension pages from scoped registries:

- `activities`: product baseline is `home`, `plugins` (or `settings`), and `system`. Extensions may register extra first-level activities; duplicate ids are reused.
- `pages`: mountable components. Optional `pluginId` / `pluginIds` mark plugin-owned extra pages.
- `navigation`: links an `activityId` to a `pageId`.
- `slots`: `overview.cards`, `config.editor`, and `plugin.home`. Cards with `pluginId` also mount on that plugin's hub page. The homepage overview host lays `overview.cards` on a 4-column grid with a fixed row height; contributions declare `colSpan` / `rowSpan` (default 2×1) and only fill the card interior. Plugin hub pages still wrap contributions in `.card` using the existing flow layout.

After extensions load, the Shell fills missing `#/plugins/{pluginId}` hub pages. Extra pages stay off the plugin sidebar and are linked from the hub. Unavailable session entries are removed. Registrations that own effects must return a disposable.

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
