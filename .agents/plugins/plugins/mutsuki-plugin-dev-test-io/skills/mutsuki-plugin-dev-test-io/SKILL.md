---
name: mutsuki-plugin-dev-test-io
description: Use when driving dev-only Mutsuki process I/O or JSONL protocol smoke tests through the bundled MCP tools.
---

# Mutsuki Dev Test I/O

Use this skill to run local commands, drive a long-running stdio process, or
send JSONL requests while testing Mutsuki behavior.

## Boundaries

- This plugin is a dev-only test harness, not a runtime effect runner or standard Mutsuki plugin.
- Keep MCP, process handles, sockets, and test-runner objects out of Rust
  `contracts` and `core`.
- Prefer `jsonl_request` for Mutsuki stdio JSONL runner checks.
- Prefer `run_command` for one-shot validation commands.
- Use `start_process`, `write_stdin`, `read_output`, and `stop_process` only
  when the process must stay alive across multiple inputs.

## Safety

- Keep commands scoped to the current repository unless a test explicitly needs
  another directory.
- Set timeouts and output limits for commands that may hang or produce large
  output.
- Treat non-zero exits, timeouts, malformed JSONL, and missing response ids as
  test evidence instead of swallowing them.
