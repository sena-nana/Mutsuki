---
name: workflow-observe-plugins
description: Implement or change standard linear or broadcast workflows, observation sinks, logging plugins, trace propagation, fan-out, or aggregation behavior.
---

# Workflow And Observe Plugins

- Express workflow work as tasks and runner results; do not create a private scheduler or queue.
- Preserve trace, correlation, ordering, cancellation and per-entry completion through fan-out and aggregation.
- Keep observe payloads structured and redact secrets before logs or sinks.
- Let Host aggregate lifecycle health; plugins only report their own domain state.

Test multi-entry ordering, partial branch failure, cancellation, trace propagation and redaction.
