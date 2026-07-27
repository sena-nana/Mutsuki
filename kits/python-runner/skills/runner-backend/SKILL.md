---
name: runner-backend
description: Change PythonRunnerBackend, runner registration, async or scalar adapters, batch invocation, task.call, cancellation, disposal, context propagation, or plugin implementation binding.
---

# Runner Backend

- Execute only batches supplied by Core and return one completion per entry.
- Keep entry decode and execution failures isolated; never convert failure into a default success.
- Make `ctx.call` submit a Core task and return task-handle semantics instead of calling another plugin locally.
- Preserve generation, trace, correlation, deadline and cancellation context.
- Keep decorators and adapters as authoring glue, not a scheduler or registry.

Test single and multi-entry execution, partial decode failure, async cancellation and disposal.
