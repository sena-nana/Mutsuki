---
name: event-routing-command
description: Change generic Bot event routing, subscriptions, command parsing, command dispatch, matching, reply task generation, or business-facing Bot Runner behavior.
---

# Event Routing And Command

- Consume generic Bot events and emit generic Bot tasks; never call a platform API directly.
- Keep subscriptions and commands declared in manifests and RunnerDescriptors.
- Implement only batch-first `run_batch`; isolate decode/handler failure per entry.
- Propagate target, sender, reply, trace and correlation context into generated tasks.
- Keep durable business state in declared Store/Repository capabilities, not router internals.

Test single/multi-entry dispatch, partial decode failure, precedence, no-match and reply task shape.
