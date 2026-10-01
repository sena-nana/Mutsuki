---
name: naming-guard
description: Check Mutsuki component names against current ownership boundaries when adding or renaming crates, modules, types, or traits.
---

# Naming Guard

Read the root and scoped architecture instructions before judging a name. Name a component by the boundary
it owns, and split mixed responsibilities instead of adding aliases.

Use these distinctions: `Core` for runtime facts and scheduling; `Host` for application environments;
`Backend` for execution forms; `Bridge` for boundary conversion; `Adapter` for external or legacy translation;
`Protocol` for pure contracts; `SDK` for author helpers; `Service` for host-provided capabilities;
`Plugin` for replaceable behavior; `Provider` for capability implementations; `Gateway` for permission and
side-effect exits; `Store`/`Repository` for persistence.

Reserve `Host` for CLI, Service, Tauri, Web, Test, or Distributed environments. Do not put policy in a
Bridge, storage in a Protocol, scheduling in an SDK, or business behavior in a product assembly crate.
Update the owning `AGENTS.md` and skill whenever a new boundary is introduced.

For a multi-component audit, read [naming rules](references/naming-rules.md).
