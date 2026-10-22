---
name: web-host
description: Change WebHost lifecycle, bridge, extension loader, recovery, security, or host tests.
---

# Web Host

Use for `crates/mutsuki-web-{host,protocol,bridge,extension,recovery}`.

## Boundary

- Own HTTP/WS lifecycle, static assets, one `WebApplication`, optional extensions.
- Frontend talks only through typed RPC/Event envelopes; capability checks are server-side.
- Static responses set `Content-Security-Policy`. Baseline `img-src` is `'self' data: blob:`;
  applications may add extra https image hosts via `WebApplication::extra_img_src`, and
  WebExtensions may add hosts they need via `WebExtension::extra_img_src`. Host unions both.
- Tokens authenticate once and never appear in logs, errors, or extension APIs.
- Browser login: `POST /auth/session {"token"}` → `204` + `mutsuki_session` cookie
  (`HttpOnly; SameSite=Strict; Path=/`, `Secure` under TLS, idle TTL from
  `CookieSessionPolicy`); `401 {"code":"auth_failed"}` on a bad token. `POST /auth/logout`
  revokes it. Cookie ids are random, in memory only, never derived from the token.
- `/ws` and the auth endpoints refuse a present `Origin` that does not name the request host.
  A hello without a token authenticates with the connection's cookie; an explicit token wins.
- Error frames use `ProtocolError::code()` (`auth_failed`, `unauthenticated`,
  `capability_denied`, `version_mismatch`, …) and carry the failing `request_id` or
  `subscription_id`. A refused hello closes the socket.
- Recovery Shell: status, load failures, disable extension, clear cache, safe mode, minimal errors — not a plugin admin console.
- Default listen is loopback; non-loopback requires TLS or explicit remote auth.
- Standalone mode requires validated `link_endpoint` (`local://` only; not a Bot product path).
- Axum/Hyper types stay inside `server.rs` and are not public ABI.

## Rules

- Validate extension manifest version, protocol compatibility, entry, and content hashes.
- Isolate setup failures; dispose registrations on unload/disable.
- Event fanout uses bounded queues with drop metrics; idle sockets must not busy-poll.
- Missing config/capability/revision fails structured.
- Assert behavior in tests (status, HTTP codes, RPC, budgets), not log text.
