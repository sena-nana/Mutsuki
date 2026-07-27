---
name: io-effect-plugins
description: Implement or change standard filesystem, HTTP client, configuration permission, or other external I/O and effect gateway plugins.
---

# I/O And Effect Plugins

- Translate generic protocol tasks into real external effects only in effectful runners or gateways.
- Validate permission scope, target and resource descriptors before I/O.
- Obtain credentials and secrets through Host services; never store or log raw values.
- Return structured status and errors; do not silently substitute mock or unavailable backends.

Test allow/deny paths, partial failures, cancellation, redaction and external-boundary fakes.
