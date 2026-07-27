---
name: terminal-ui
description: Change MutsukiCliHost ratatui or crossterm rendering, keyboard input, refresh flow, terminal session setup or cleanup, status presentation, navigation, or interactive lifecycle.
---

# Terminal UI

- Render only state returned by the control client and label stale/unavailable data clearly.
- Keep rendering pure where practical and separate input actions from control requests.
- Restore terminal mode on normal exit, error, panic and Ctrl+C paths.
- Avoid exposing tokens, secrets or raw sensitive payloads.
- Do not add product business panels unsupported by the ServiceHost control API.

Test state transitions and terminal cleanup rather than decorative text snapshots.
