---
name: mutsuki-naming-guard
description: Enforce Mutsuki/NanoBot component naming and responsibility boundaries. Use when Codex works in Mutsuki or NanoBot repositories, adds or renames crates/modules/types/traits, audits Host/Backend/Bridge/Services/Protocol/SDK/Plugin/Core/Adapter/Provider/Gateway naming, or checks whether components have unclear mixed responsibilities.
---

# Mutsuki Naming Guard

## Workflow

1. Read the repository instructions and current architecture/contracts docs before judging names.
2. Classify each new or touched component by the question it answers, not by what it feels like.
3. If a name does not match the component's answer, rename or split at the correct boundary instead of adding aliases.
4. If responsibilities cross categories, split the component or move the behavior to the existing layer that owns it.
5. For code changes, update docs/contracts first when the boundary is new or not yet described.

## Decision Order

Ask these in order:

1. Is it the minimal scheduling fact source? Use `Core` or `Kernel`.
2. Does it let Mutsuki run inside an application environment? Use `Host`.
3. Does it decide how plugins or runners are loaded/executed? Use `Backend` or `Loader`.
4. Does it connect two runtime, protocol, process, or UI boundaries? Use `Bridge`.
5. Does it translate an external or legacy system into current protocol? Use `Adapter`.
6. Does it only declare task/resource/event/effect types? Use `Protocol`.
7. Does it provide author-facing helper APIs? Use `SDK`.
8. Does it expose host capabilities to plugins? Use `Services` or `ServiceRegistry`.
9. Does it implement replaceable behavior? Use `Plugin`.
10. Does it provide an external capability implementation? Use `Provider`.
11. Does it control permissions or external side effects? Use `Gateway`.
12. Does it persist local state or CRUD data? Use `Store` or `Repository`.

## Mandatory Rules

- Reserve `Host` for application runtime environments such as Tauri, CLI, Service, or Test.
- Use `Backend` for Native, ABI, JSONL, Python, WASM, process, or sidecar execution forms.
- Use `Bridge` only for boundary transfer/conversion; move policy choices to `Policy` or `Router`.
- Use `Services` only for host-provided capability sets consumed by plugins.
- Use `Protocol` only for pure contracts; no providers, storage, Tauri, UI, or SDK calls.
- Use `SDK` only for plugin-author facade/helpers; it must lower to Task, ResourceRef, plans, and RunnerResult.
- Use `Plugin` only for replaceable behavior implementations.
- Use `Core` only for runtime kernel or a product-domain plugin set.
- Use `Adapter` for external/legacy protocol translation.
- Use `Provider` for concrete suppliers of external capabilities.
- Use `Gateway` for permission checks, policy enforcement, audit, and side-effect exits.

## Reference

Read [references/naming-rules.md](references/naming-rules.md) when auditing more than one component, reviewing a proposed rename, or deciding whether to split responsibilities.
