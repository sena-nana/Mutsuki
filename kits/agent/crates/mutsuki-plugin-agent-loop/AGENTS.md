# Agent Loop Plugin Instructions

- The loop plugin owns run and step orchestration behavior.
- Do not claim multi-step tool execution, approvals, or long-term memory as complete unless the runner path actually performs it.
- Keep deterministic loop behavior in `src/loop.rs`; keep runtime registration glue in `src/plugin.rs`.
