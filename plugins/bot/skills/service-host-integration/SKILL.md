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
- Business EventSources submit through `BotSubmissionGate` (their own poll/ingest protocols only,
  never platform business protocols), business manifests pass
  `BotSubmissionGate::ensure_manifest_business_surface` before registration, and the Flow router
  assembly publishes its ingress stats on the `mutsuki.bot.flow.ingress` health probe.

Test the real `ServiceRuntimeBuilder` assembly path, startup failure, health and graceful shutdown.
