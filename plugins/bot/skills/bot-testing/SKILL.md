---
name: bot-testing
description: Add or change Bot protocol conformance, Runner batch tests, fake platform servers, adapter integration tests, ServiceRuntime end-to-end tests, smoke accounts, health, reconnect, or shutdown verification.
---

# Bot Testing

- Unit-test DTOs and pure routing; integration-test through real Runner and task-routing surfaces.
- Keep Core, ServiceRuntime and ResultRouter in end-to-end paths; fake only external platform boundaries.
- Cover single/multi-entry batches, partial failure, task handles, trace/correlation and generation propagation.
- Keep real credentials in ignored local configuration and redact captured output.
- Verify stop releases sockets, workers and EventSources.
- Cover the Flow-only business surface: `BotSubmissionGate` rejections (send/recall/delivery/agent
  direct submissions), manifest validation at assembly, and ingress stats accounting
  (accepted/dropped, self-sent skipped, `matched_sources` in ingress output).

Report whether validation used unit, fake-server or real-account smoke coverage.
