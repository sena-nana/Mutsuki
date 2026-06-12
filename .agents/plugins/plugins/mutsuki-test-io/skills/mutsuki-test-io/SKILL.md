---
name: mutsuki-test-io
description: Use when driving Mutsuki backend or process I/O tests from Codex through the bundled MCP tools.
---

# Mutsuki Test I/O

Use this skill when Codex needs to run local commands, drive a long-running
stdio process, or send JSONL requests while testing Mutsuki behavior.

## Boundaries

- This plugin is a test harness, not a StrategyBackend.
- Keep Codex, MCP, process handles, sockets, and test-runner objects out of
  Rust `contracts` and `core`.
- Prefer `jsonl_request` for Mutsuki stdio backend checks.
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
