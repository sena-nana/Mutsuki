---
name: integration-testing
description: Validate MutsukiCliHost against ServiceHost control APIs, IPC authentication, real status and health, task controls, log tailing, disconnects, shutdown, or terminal-session cleanup.
---

# Integration Testing

- Exercise the same control-client path used by the terminal UI.
- Prefer a real ServiceHost for product integration; protocol fakes may replace only the IPC server boundary.
- Verify auth failure, unavailable components, refresh, disconnect/reconnect and graceful exit.
- Assert structured state and actions, not log wording or terminal decoration.
- Confirm tests do not emit tokens and terminal modes are restored.

Record the ServiceHost revision used by integration coverage.
