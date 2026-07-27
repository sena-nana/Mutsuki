# DistributedHost deployment contract

Distribution is an optional external deployment around the local Bot product. The product still
starts exactly one ordinary `ServiceRuntime`; `MutsukiDistributedHost` is installed and supervised
by the deployment system as a separate process or service. No `ClusterContext`, `NodeId`, trust
level, scheduler API or distributed metric is injected into a plugin.

## Selecting a mode

The committed product template is explicit and inert:

```toml
[distribution]
mode = "disabled"
```

`disabled` does not read a deployment file and the template starts no sidecar process, listener,
distributed telemetry or replica work. `local_observable` and `clustered` require an explicit file
relative to the product config:

```toml
[distribution]
mode = "clustered"
deployment = "../deploy/distribution/controller-worker.toml"
acceptance = "fast"
fallback = "reject"
```

The schema-v2 deployment file must pin the same DistributedHost release and revision as the template,
require `deployable` maturity and explicit feature flags, name only secret key references, and name
both the ordinary local ServiceHost endpoint and the Link-local sidecar management endpoint. Before
starting ServiceRuntime, the product authenticates that already-running endpoint and verifies the
capability schema/protocol version, release, revision, aggregate maturity, feature proof and health.
It still never starts or supervises the sidecar. Authenticated/encrypted control and data channels,
resource budgets, and direct data transfer off the Leader remain mandatory. Missing files, unknown fields, mismatched revisions,
invalid topology, raw-looking secret references, insecure channels and zero required budgets fail
before the local `ServiceRuntime` starts.

Only `acceptance = "fast"` may opt into `fallback = "local_degraded"`. That state is reported by the
ServiceHost `health` component as local fallback with `remote_execution = false`; the monitor retries
an authenticated handshake and reports recovery. `durable` and `critical` always reject local fallback.
`local_observable` also reports `remote_execution = false` and never claims remote task execution.

Committed, machine-neutral examples are:

- `deploy/distribution/single-node.toml`: one node with voter and Worker roles.
- `deploy/distribution/controller-worker.toml`: one controller plus a separate Worker.
- `deploy/distribution/ha-three-voters-worker.toml`: three voters plus a Worker.
- `deploy/distribution/task-policies.toml`: policy examples shared by every topology.

Replace `.example.invalid` endpoints only in local/deployment configuration. Do not commit account,
certificate, private-key, token, machine path or secret values.

## Local and distributed status

`observability.local_service_health` identifies the existing ServiceHost health source;
`observability.cluster_health_endpoint` identifies the external cluster source. Operators display
them side by side because one cannot stand in for the other. Plugins continue to emit only their
ordinary local health and metrics.

| Cluster state | User-visible meaning | Safe action |
| --- | --- | --- |
| `Healthy` | Quorum, control lease and required replicas are available. | Admit according to task policy. |
| `Degraded` | Useful work remains possible but capacity, replicas or redundancy are reduced. | Preserve local realtime work; reduce background distribution. |
| `QuorumLost` | No controller majority can commit durable state. | Keep valid grants running; reject Durable/Critical acceptance. |
| `Isolated` | This entry/Worker cannot authenticate or reach the active control plane. | Run only explicitly LocalOnly Fast work or reject. |
| `RecoveryRequired` | A task/effect cannot be safely retried or committed automatically. | Hold for verifier, compensation or operator decision. |
| `Quarantined` | Identity, artifact, receipt or result integrity is suspect. | Revoke grants, isolate outputs and trace affected tasks. |

## Failure drills

Run drills with synthetic tasks and non-production secrets. Record the GlobalTaskId, attempt, term,
grant expiry and content IDs before and after each drill.

| Drill | Inject | Required observation | Recovery |
| --- | --- | --- | --- |
| Leader loss | Stop the active voter. | Existing unexpired grants continue; no new state uses the old term. | Majority elects a Leader; clients rediscover it; restarted old Leader is fenced. |
| Worker loss | Stop a Worker during execution. | Attempt becomes suspect/dead; no duplicate commit occurs. | Restart/restore only for eligible mobility, input and effect policy; otherwise fail or require action. |
| Entry loss | Stop the submitting node after durable receipt. | Task remains queryable by GlobalTaskId. | Reconnect through another entry and observe the same durable record. |
| ServiceHost loss | Stop only local `mutsuki-bot`. | External cluster remains alive; local plugins and IPC stop cleanly. | Restart ServiceHost; reconcile local handles without inventing acceptance. |
| Sidecar loss | Stop only DistributedHost. | Local ServiceRuntime stays healthy; distributed state becomes unavailable. | Fast tasks follow explicit local/reject fallback; Durable/Critical are rejected until recovery. |

## Diagnosis and repair order

1. **Replica shortage:** compare required acceptance copies with healthy content/metadata replicas.
   Stop pre-replication first under pressure; never report Durable until its receipt requirements hold.
2. **Staged output not committed:** verify the active attempt, output digest, required copies and
   current term. A staged result is not user-visible committed output.
3. **Damaged checkpoint:** verify schema, generation, baseline and changed-chunk hashes. Restore an
   earlier valid checkpoint or restart from durable input only when the recovery/effect policy allows.
4. **Old Attempt result:** compare attempt and grant term with the durable record. Reject stale output;
   never let completion from an old attempt overwrite the active one.
5. **Verification conflict:** quarantine candidate output, preserve both receipts and audit records,
   apply the configured verifier/replay/N-of-M policy, then commit, recompute or require review.

When a node rejoins, reconcile active tasks and grants first, then staged outputs/checkpoints, and
only then repair ordinary replicas in the configured CPU/network/checkpoint budgets. Large input,
output, model, checkpoint and stream bytes always travel directly between origin/storage and Worker;
the Leader handles bounded control descriptors only.

## Task policy semantics

The catalog demonstrates DistributedHost-owned mobility, recovery, latency, acceptance, retry
safety, quality, cache and partial-result choices. The template checks that the referenced catalog is
present and non-empty; it does not duplicate those runtime semantics. Fast is the preview/BestEffort
acceptance level. Every example explicitly states quality bounds, cache, partial results and fallback,
so degradation is never silent. Confidential, restricted and credential-bearing policies require at
least Managed trust; that decision is made by DistributedHost and is not observable through the
plugin API.
