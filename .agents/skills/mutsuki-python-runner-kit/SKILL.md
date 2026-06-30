---
name: mutsuki-python-runner-kit
description: Work on MutsukiPythonRunnerKit, the Python SDK and runner glue for Mutsuki Runner Link. Use when editing the split Python repository, its AGENTS.md, contracts mirror, runner backend, resource helpers, transport bridge, manifest/CLI, or conformance tests.
---

# Mutsuki Python Runner Kit Skill

Read `AGENTS.md` first, then use the smallest matching operation below.

## Operations

| Operation | Scope | Required context |
| --- | --- | --- |
| Boundary docs | `README.md`, `AGENTS.md`, `plans/` | `plans/repository-boundary.md`, `plans/runner-link-v1.md` |
| Contract mirror | `src/mutsuki_runner_kit/contracts/` | matching contract module, `tests/test_contracts_*.py` |
| SDK facade | plugin-author APIs such as `Plugin`, `Context`, decorators, `ctx.call`, `ctx.resources` | relevant SDK/runtime module and behavior tests |
| Runner backend | registration, invocation, cancel, dispose, async adapter | `src/mutsuki_runner_kit/runners/`, `tests/test_backend.py`, `tests/test_async_adapter.py` |
| Transport bridge | stdio JSONL, framing, codec, Runner Link methods | `plans/runner-link-v1.md`, `src/mutsuki_runner_kit/transport/`, `tests/test_stdio.py` |
| Resource helpers | resource clients, manager, plans, leases, generation checks | matching resource module, `tests/test_resource.py`, `tests/test_contracts_resource.py` |
| Manifest or CLI | manifest parse/validate/export, inspect/run/dev commands | `plans/repository-boundary.md`, existing manifest/CLI tests if present |
| Tests or conformance | tests, fake Core, golden frames | module under test and existing behavior tests |

## Rules

- This repository is Python Runner SDK + Runtime Kit, not Host, Core, Agent kit,
  workflow engine, plugin marketplace, or virtualenv manager.
- Core remains the authority for TaskPool, scheduling, registry, state, event,
  trace, resource lifecycle, and load-plan validation.
- Python mirrors Core wire shape; do not invent Python-only protocol fields.
- SDK helpers must lower to `Task`, `TaskAwait`, `RunnerResult`, `ResourceRef`,
  `ValueRef`, and resource plans.
- `ctx.call` creates a Core-routed task; it must not locally invoke another
  plugin.
- Resource and value handles are descriptors, not Python object handles.
- Fail loudly with structured errors for missing fields, unsupported awaitables,
  stale generation, lease mismatch, or runner management failure.
- Tests should assert behavior and protocol semantics, not exact log strings or
  incidental implementation text.

## Naming

- `SDK`: plugin-author helpers.
- `Backend`: runner execution form.
- `Bridge`: boundary transport or codec conversion.
- `Protocol` / `contracts`: pure wire objects.
- Do not use `Host` in this repository.

## Validation

For code or contract changes:

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```

Documentation-only changes may use targeted review, but do not report them as
runtime-validated.
