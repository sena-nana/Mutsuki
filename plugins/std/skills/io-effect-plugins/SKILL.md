---
name: io-effect-plugins
description: Implement or change standard filesystem, HTTP client, configuration permission, or other external I/O and effect gateway plugins.
---

# I/O And Effect Plugins

- Translate generic protocol tasks into real external effects only in effectful runners or gateways.
- Validate permission scope, target and resource descriptors before I/O.
- Obtain credentials and secrets through Host services; never store or log raw values.
- Return structured status and errors; do not silently substitute mock or unavailable backends.

HTTP client (`mutsuki-plugin-io-http-client`, protocol `mutsuki.http.request@2`):

- Callers never put a credential in the request. `secret_headers` names Host secret keys and
  `capture_cookies` stores response `Set-Cookie` values into one; both are rejected
  (`http.secret_denied`) unless `HttpClientConfig.secret_grants` grants that key for the URL's
  domain (`capture: true` for capture). Values are resolved when the effect runs, and secret
  headers are dropped when a redirect leaves the host.
- `set-cookie` never appears in returned headers. `response_body: json` returns the body inline
  (capped at `MAX_INLINE_JSON_BYTES`), and `select` narrows it to JSON pointers so sensitive fields
  stay out of task output. The `DomainEvent` carries metadata only.
- The ServiceHost factory binds `HttpSecretStore` to `HostSecretStore`; another Host must bind
  its own store or leave secrets unavailable.

Test allow/deny paths, partial failures, cancellation, redaction and external-boundary fakes.
