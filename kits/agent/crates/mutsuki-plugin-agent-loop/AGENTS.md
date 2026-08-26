# Agent Loop Plugin Instructions

- The loop plugin owns run and step orchestration behavior.
- Do not claim multi-step tool execution, approvals, or long-term memory as complete unless the runner path actually performs it.
- Keep deterministic loop orchestration in `src/plugin.rs`; keep turn fencing and profile storage in `mutsuki-agent-runtime` (`AgentLoop`).
