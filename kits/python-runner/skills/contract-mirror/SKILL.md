---
name: contract-mirror
description: Mirror MutsukiCore runtime contracts in Python, including tasks, batches, entries, runners, resources, effects, events, plugin surfaces, errors, codecs, and validation.
---

# Contract Mirror

- Treat the pinned MutsukiCore wire shape as authority; do not invent Python-only semantics.
- Preserve field names, optionality, enum values, defaults and structured error codes.
- Keep `WorkBatch`/`CompletionBatch` batch-first and require exactly one outcome per entry.
- Represent tasks with handles and resources with descriptors, never live Python objects or clients.
- Update codecs, exports and round-trip tests with every mirror change.

Record the source Core revision used for a contract update.
