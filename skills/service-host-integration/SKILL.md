---
name: service-host-integration
description: Change Bot plugin bundles, manifests, native runner factories, health probes, EventSource registration, secret binding, ServiceRuntimeBuilder integration, or product-host assembly helpers.
---

# ServiceHost Integration

- Expose reusable bundle/install APIs; do not create a host process or own application lifecycle.
- Register real manifests, runners, EventSources and health probes before ServiceRuntime freezes its plan.
- Keep secrets as Host references and populate credentials only at the Host boundary.
- Ensure declared capabilities and deployment match installed implementations.
- Return unavailable on missing upstream capability instead of registering placeholder health or runners.

Test the real `ServiceRuntimeBuilder` assembly path, startup failure, health and graceful shutdown.
