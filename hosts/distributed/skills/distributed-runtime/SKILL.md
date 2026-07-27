---
name: distributed-runtime
description: Change Mutsuki distributed contracts, task placement, host adapter, control client, trust, replication, or sidecar assembly. Use for code under hosts/distributed that affects distributed execution without changing Core semantics.
---

# Distributed Runtime

- Keep node, cluster, quorum, placement and remote resource facts out of Core and generic SDK packages.
- Translate remote work to ordinary local Task/Resource contracts only at the explicit host adapter.
- Require typed request identity, idempotency, fencing, timeout and structured failure.
- Keep Link transport replaceable and Service control access authenticated.
- Reject clustered/HA profiles until every selected process, endpoint and recovery component is installed.
- Test local-only Core behavior to prove distributed support remains zero-intrusion.
