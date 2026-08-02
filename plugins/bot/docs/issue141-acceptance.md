# Issue #141 acceptance matrix

This matrix is the closure gate for the QQ Adapter scope. It does not claim support for another
platform or for QQ operations that have no implemented endpoint. Public QQ networking is replaced
only at the external boundary; ServiceRuntime, EventSource, task routing, adapter runners and
resource validation remain real.

| Criterion | Owner implementation | Executable evidence |
| --- | --- | --- |
| Private, group and channel inbound | Gateway event allowlist and QQ event mapper | `qqbot_issue141` real ServiceRuntime + scripted fake HTTP/WebSocket E2E |
| Mention, reply and quote | `BotEvent` normalization and QQ segment mapping | E2E channel assertions plus adapter mapping tests |
| Image, audio and file inbound `ResourceRef` | `QqGatewayMediaHandler` and injected resource gateway | loopback download tests cover private/group/channel and image/audio/video/file; every ref has size and a non-empty content hash |
| Text, image, audio and file outbound | `QqOpenApiService` Group/C2C message and separated upload paths | adapter request/ordering tests cover text/image/audio/video/file, explicit upload and Reply/Quote-to-`msg_id` lowering |
| Truthful capability matrix | QQ protocol DTO + `QqBotConfig::capability_matrix` | intent/provider tests cover conversation/segment kinds, optional media, size limits, server-driven rate policy, retry configuration, configured intents/shard and permission requirements |
| Resume without duplicate delivery | `QqGatewayPump` bounded event-kind/message-id dedup and ServiceHost Resume | scripted fake Gateway reconnects with op 6, replays a group message, and observes one business delivery |
| Rate limit, token refresh and permanent error distinction | `QqOpenApiTransport` and structured adapter task failures | 401 refresh-once, bounded 429/5xx retry, retry-after and stable `qqbot.openapi.*` error evidence tests |
| Health, degraded state and trace | QQ Gateway health snapshot, ServiceHost event-source supervisor and structured tracing | health transition test covers disconnected/degraded/healthy/reconnect states; ServiceHost fake E2E verifies healthy component after Identify/Resume; submitted frames trace account/session/event/sequence/correlation |
| Business code does not call QQ OpenAPI | standard `mutsuki.bot.*` task boundary | fake E2E business capture consumes only `BotEvent`; existing echo/AI E2E sends through `mutsuki.bot.message/send@1` |
| Performance | Bot performance model v2 | `bot.event-burst-10k`, `bot.duplicate-event`, `bot.reconnect`, `bot.rate-limit`, `bot.connection-idle` and `bot.long-run`; the full declared acceptance budget must pass |

Gateway message create/delete dedup uses `(event kind, message id)` when a message ID exists. This
suppresses reconnect replays even if the envelope event ID changes, while allowing a later delete
event for the same message to reach business code. The normalized `qqbot.sequence` field preserves
the Gateway ordering fact.

Channel support is deliberately inbound-only. Group/C2C are the only standard outbound target
kinds currently declared, and message edit remains unsupported. Missing media providers remove
media protocols, segments and permissions from the manifest/capability surface rather than
installing a fallback.

Verification layers:

- Unit/owner: Bot protocol, QQ adapter, testkit, QQ Web capability consumer.
- Fake HTTP/WebSocket E2E: real ServiceRuntime, Identify, heartbeat, reconnect, Resume, all three
  conversation kinds, replay suppression and graceful close.
- Performance: versioned Bot workload/report with all budgets evaluated.
- Real account: not claimed; credentials and public QQ smoke remain an external release gate.

Protocol audit revisions: Tencent `tencent-connect/openclaw-qqbot`
`47142c997bdbc9e72d92b817ff378941b3be7d4c` and Tencent `tencent-connect/botgo`
`fe31c0dfe469001e0f783d2f07e7de7bd08b403f`.

## Recorded local acceptance

Final acceptance was run on 2026-08-02 (Asia/Shanghai) from dirty revision
`f48683e5136c89c20f7c6931b2501f3a48bf2f5d`; the dirty files are exactly the Issue #141 change set.

- `cargo test --workspace --all-targets --locked`: 1,122 passed, 3 ignored, 9 filtered out
  across 207 suites.
- Modified Bot owner/consumer packages in an independent local clone with no sibling repositories:
  53 passed across 8 suites after a fresh 603-crate `cargo check --workspace --all-targets --locked`.
- `python3 skills/monorepo-maintenance/scripts/check_workspace.py`, locked metadata, `cargo fmt`,
  `scripts/check-distributed-boundary.sh`, full Workspace check and `git diff --check`: passed.
- Strict Clippy with `--no-deps -D warnings` for the modified protocol, QQ adapter, testkit and
  ServiceHost integration packages: passed.
- `cargo bench-smoke`: 59/59 Core smoke gates passed.
- `python3 scripts/run-performance-model.py --mode reference --process-runs 3 --output
  artifacts/performance/issue141-reference.json`: correctness passed with zero duplicate executions,
  hash mismatches, public-network requests, unexpected errors or wrong routes; performance acceptance
  was complete and passed all 9/9 declared budgets. `bot.event-burst-10k` p95 was 22.241 ms against a
  50 ms budget.

One earlier default-parallel Workspace run observed the unrelated
`abort_before_native_worker_start_skips_domain_handler` runtime-host timing failure. Its exact and
single-thread package reruns passed, no runtime-host file is changed here, and the final unchanged
default-parallel Workspace command passed in full. No gate was weakened or skipped.

The ignored real-account QQ smoke was not run because no public-account credential was supplied. It
remains an external release smoke and is not represented by the fake-server evidence.
