---
name: service-host-integration
description: Change Bot plugin bundles, manifests, native runner factories, health probes, EventSource registration, secret binding, ServiceRuntimeBuilder integration, or product-host assembly helpers.
---

# ServiceHost Integration

- Expose reusable bundle/install APIs; do not create a host process or own application lifecycle.
- Register real manifests, runners, EventSources and health probes before ServiceRuntime freezes its plan.
- Keep secrets as Host references and populate credentials only at the Host boundary.
- Keep ServiceHost dependencies in the dedicated integration crate, never in platform adapter crates.
- Ensure declared capabilities and deployment match installed implementations.
- Return unavailable on missing upstream capability instead of registering placeholder health or runners.
- Open `data/bot/state.sqlite3` once per runtime and share the `BotStateDbRepository` actor
  across Sandbox, QQ, Agent and interaction factories through the ServiceHost application
  service `mutsuki.bot.state` (`bot_state_db(builder)`, built on
  `ServiceRuntimeBuilder::ensure_application_service`); the sandbox simulator is shared the same
  way (`mutsuki.bot.sandbox.state`). Never keep owner state in factory `OnceLock` slots or rely
  on selection order, and do not call `BotStateDbRepository::open` again on the same path.
- The sandbox adapter (`mutsuki.plugin.bot.adapter-sandbox`) needs the shared `BotFlowRegistry`:
  it installs `SandboxFlowRuntime` (compose into ingress, live send through `send@2`) and its
  live ingress observer on that registry, and removes the observer through a host effect on
  unload. Another adapter's integration may only feed it optional enrichment (QQ's group-title
  sink); never wrap an adapter's runners or delivery gateway for the sandbox.
- A hand-built Flow assembly registers the ingress, match, node and outbound runners
  (`flow_outbound_runner`) and activates each LoadPlan on the registry; the node and outbound
  runners are plan-bound and rebuild after `plan_generation` moves.
- Register Host recovery EventSources for reply delivery `ResumeDue`, active delivery
  `ResumeDue`, and interaction `Recover`. Completion paths drain persisted work; they do not
  originate new business behavior. Active-delivery runner and recovery source share the QQ
  adapter plugin id so the product load plan keeps them; do not register
  `mutsuki.plugin.bot.delivery` as a separate configured plugin.
- Business EventSources submit through `BotSubmissionGate` (their own poll/ingest protocols only,
  never platform business protocols), business manifests pass
  `BotSubmissionGate::ensure_manifest_business_surface` before registration, and the Flow router
  assembly publishes its ingress stats on the `mutsuki.bot.flow.ingress` health probe.

Test the real `ServiceRuntimeBuilder` assembly path, startup failure, health and graceful shutdown.
