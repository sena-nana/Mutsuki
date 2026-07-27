---
name: control-client
description: Change MutsukiCliHost ServiceHost control protocol client, IPC transport, authentication, request or response handling, status models, refresh, task operations, log tailing, or connection errors.
---

# Control Client

- Consume only published ServiceHost control/IPC contracts through `ControlClient`; never load server `ServiceConfig`.
- Authenticate every non-test connection and keep tokens out of display and errors.
- Preserve structured control errors and `TaskHandle` semantics for cancel/outcome operations.
- Treat disconnect, unsupported method and unavailable backend as explicit states.
- Do not call Core or plugin internals as a fallback.

Test authentication, request mapping, malformed responses, timeout and reconnection.
