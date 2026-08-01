# Issue #140 acceptance matrix

This matrix records the owner implementation and executable evidence used to close the QQ AI Bot
epic. Public QQ networking is replaced only at the external boundary; runtime, task routing,
continuations, repositories, media bridge, Agent bridge, and delivery logic remain real.

| Issue | Required result | Owner evidence | Functional evidence |
| --- | --- | --- | --- |
| #141 | private/group/channel, quote/mention/media, truthful capabilities and reconnect | `mutsuki-plugin-bot-adapter-qqbot` | adapter tests, inbound-media loopback E2E, service-host fake QQ E2E |
| #142 | typed commands, composable guards, permission/rate limit, priority/propagation and isolation | protocol, command and event-router crates | command/parser and handler-pipeline behavior tests |
| #143 | stable ConversationRef, inherited policy and durable fenced Agent binding | conversation and state-db crates | policy/session tests and SQLite reopen test |
| #144 | public Agent client bridge, two-turn reuse, stream/final, cancel/reset/fork/regenerate and replay suppression | `mutsuki-plugin-bot-agent` | bridge tests and ServiceRuntime QQ AI E2E |
| #145 | validated ResourceRef media, STT/TTS and generic MediaService transcode | QQ adapter, Bot media bridge and Agent bridge | inbound-media fake server tests and ServiceRuntime audio→STT→Agent→TTS→QQ E2E |
| #146 | durable idempotent active delivery and scheduled Agent result bridge | delivery and state-db crates | retry/restart tests plus ServiceRuntime ScheduledRunResult→QQ replay-suppression E2E |
| #147 | durable multi-step non-Agent interaction with recovery and propagation | interaction and state-db crates | state-machine/reopen tests plus ServiceRuntime two-step consume E2E |
| #148 | QQ domain WebExtension for live provider injection, typed actions, permissions, revision, audit and secret redaction | `mutsuki-plugin-bot-qq-web` | WebHost WebSocket RPC E2E and frontend syntax check |

The full ServiceRuntime E2E is
`crates/mutsuki-bot-service-host-integration/tests/qq_ai_pipeline.rs`. It additionally proves that
the command parser and Handler pipeline are registered through their public protocol bindings.

Performance acceptance is versioned in `benchmarks/workloads-v2.json`; the reference invocation
must report `correctness.passed=true`, `performance_acceptance.passed=true`, and
`performance_acceptance.complete=true` with all declared budgets evaluated.

No real-account QQ smoke is claimed because the repository contains no account credentials. The
fake servers exercise real HTTP/WebSocket and media-download boundaries without public requests;
a credentialed smoke remains release-environment evidence rather than a substitute for these
deterministic gates.
