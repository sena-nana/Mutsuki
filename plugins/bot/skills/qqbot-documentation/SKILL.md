---
name: qqbot-documentation
description: Write or update QQBot adapter documentation, configuration guidance, support matrices, ServiceHost assembly instructions, health and troubleshooting notes, fake-server verification, real-account smoke procedures, or official QQ protocol compatibility records.
---

# QQBot Documentation

- Verify current QQ Open Platform documentation, an official Tencent reference implementation,
  current code, manifests and tests before describing behavior.
- Keep supported, optional and unsupported capabilities explicit. A protocol is supported only when
  the manifest, descriptor, implementation and tests agree.
- Document owner boundaries: BotPlugins owns QQ transport/mapping; ServiceHost owns lifecycle and
  secret resolution; products own selection and local configuration; business uses `mutsuki.bot.*`.
- Describe config fields and defaults without committing a complete runnable config. Use secret
  keys/refs only; never include credentials, tokens, account fixtures or sensitive logs.
- Include startup failures, health fields, reconnect/error classes, control-plane inspection and
  graceful shutdown when they are affected by the change.
- Distinguish unit, fake HTTP/WebSocket E2E and real-account smoke evidence. Never present fake
  coverage as a real QQ verification.
- Update `docs/qqbot-adapter.md`, relevant README links and issue completion evidence together; note
  the tested upstream revisions when compatibility changes.

