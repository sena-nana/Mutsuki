---
name: platform-adapters
description: Implement or change QQBot or other platform gateway connections, authentication, payload translation, OpenAPI operations, media handling, reconnect behavior, rate limits, or platform-specific protocols.
---

# Platform Adapters

- Translate platform events into generic Bot protocols and generic operations back into platform requests.
- Keep platform-only fields under the platform namespace or adapter internals.
- Obtain credentials from Host secret injection; redact tokens and platform-sensitive payloads.
- Keep transport clients and sockets inside EventSource/Gateway objects; use ResourceRef for large media.
- Report disconnect, auth, rate-limit and unsupported operation failures structurally.

Test translation, reconnect, heartbeat, retry/rate limit, media and redaction with external-boundary fakes.
