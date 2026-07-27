# Host, Link, Database and Distributed boundaries

| Mode | Public assembly boundary | AgentKit responsibility |
| --- | --- | --- |
| In-process Core | `RuntimeBootstrapper`, `RuntimeClientRef` | register manifests/runners/handlers |
| ServiceHost | runtime-client runner and async-handler factories | long-running Agent tasks and cancellation |
| TauriHost | reloadable runtime-client runner and async-handler factories | embedded tasks; Host owns event/approval bridge |
| MutsukiLink | `AgentClient` over Agent control stream | negotiation, monotonic events, reconnect policy |
| Database | `mutsuki-protocol-db` executor | checkpoint/session mapping only |
| DistributedHost | coordinator and portable task contracts | placement hints, affinity and result validation |

AgentKit does not depend on private Host APIs. ServiceHost and TauriHost assemble the same runner
descriptors; Tauri reload recreates runtime-client runners and async handlers in the new generation.
Link local IPC is the reference cross-process path, while transport/authentication remain Link
concerns.

Durable recovery restores the latest valid checkpoint under a new coordinator epoch. If the
database is unavailable, recovery is degraded read-only and side effects stay disabled. Remote
results are accepted only when task identity, generation and coordinator fencing match.

`required_resource_refs` are Host-local runtime references, not portable data descriptors.
`data_locality` constraints also cannot be proven by the current Agent portable-task request because
that request does not expose distributed direct-input descriptors. Tasks carrying either therefore
remain on the origin Host. Remote execution is limited to tasks that have no unmaterialized
resource/locality requirements and whose side effects are safe to retry. The distributed integration
test exercises the complete Coordinator → Link control envelope → WorkerEndpoint → HostAdapter
path, including outcome recovery and parent-to-subagent cancellation.
