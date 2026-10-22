---
name: platform-adapters
description: Implement or change QQBot or other platform gateway connections, authentication, payload translation, OpenAPI operations, media handling, reconnect behavior, rate limits, or platform-specific protocols.
---

# Platform Adapters

- Translate platform events into generic Bot protocols and generic operations back into platform requests.
- Keep platform-only fields under the platform namespace or adapter internals.
- Obtain credentials from Host secret injection; redact tokens and platform-sensitive payloads.
- Keep transport clients and sockets inside Gateway objects; wrap them as Host EventSources only in the integration crate. The QQ socket loop is `gateway::QqGatewayClient` (driven by `QqGatewayHost`: submitter, shutdown flag, log); change reconnect, heartbeat or close handling there, not in the wrapper. Use ResourceRef for large media.
- Require explicit media/resource providers; never install an unavailable production fallback.
- Platform plugins that fetch the web (Bilibili, Workshop, Mihuashi) hold no HTTP client: they
  await `mutsuki.http.request@2` child tasks with a per-request `domain_allowlist`, pass response
  body resources straight through, and reference credentials only as `secret_headers` /
  `capture_cookies` keys that the product grants to their domain.
- Report disconnect, auth, rate-limit and unsupported operation failures structurally.

Test translation, reconnect, heartbeat, retry/rate limit, media and redaction with external-boundary fakes.

Large media writes from AsyncBatchHandler must use `spawn_blocking` around the synchronous Host bridge, or migrate to AsyncResourceRegistryGateway; never block the async executor on SQLite.
