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

Package prefixes: `mutsuki-plugin-*` is reserved for loadable Mutsuki plugins (a `PluginBuilder`
manifest); they never depend on one another, and the Agent ones are `mutsuki-plugin-agent-*`.
AgentKit extensions loaded through the Agent extension API use `mutsuki-agent-ext-*` (API:
`mutsuki-agent-ext-api`): they are Agent extensions, not Mutsuki plugins, so the plugin-to-plugin rule
does not apply to them. Agent domain services (sessions, prompts, credentials, knowledge, ...) live in
`mutsuki-agent-services`; it is a service library, not a runtime. A package directory carries the package name, never a backend suffix
(the Takumi renderer lives in `plugins/std/plugins/mutsuki-plugin-image-render`).

Bot plugin ids follow their crate: a `mutsuki-plugin-bot-<suffix>` crate (or an integration-built
plugin) is `mutsuki.plugin.bot.<suffix>`, keeping hyphens (`mutsuki.plugin.bot.adapter-qqbot`,
`mutsuki.plugin.bot.flow`, `mutsuki.plugin.bot.sandbox`). Host service ids stay
`mutsuki.bot.<owner>.<service>` and protocol ids `mutsuki.bot.<domain>/<op>@N`; persisted Flow node
type ids never change with a rename. A plugin id is never reused as a service id.

For a multi-component audit, read [naming rules](references/naming-rules.md).
