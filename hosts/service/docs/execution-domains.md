# Execution domains

`core.execution_domains` maps product configuration to Core physical execution
domains. When the list is empty, ServiceHost preserves the legacy
compute/blocking topology selected by `core.worker_profile`.

Each `ExecutionClass` must belong to exactly one configured domain. Missing or
duplicate class ownership, zero capacity, invalid lane shares and invalid
reserved capacity fail startup instead of falling back to another worker pool.

```toml
[core]
actor_control_queue_limit = 64
actor_data_queue_limit = 512
actor_control_quota = 8

[[core.execution_domains]]
id = "interactive"
execution_classes = ["orchestration", "cpu"]
threads = 2
queue_capacity = 512
max_inflight_bytes = 33554432

[core.execution_domains.lanes.interactive]
weight = 16
reserved_entries = 8
max_share_percent = 100
queue_entry_limit = 256
max_inflight_bytes = 16777216
starvation_steps = 2
allow_idle_borrow = true

[[core.execution_domains]]
id = "background"
execution_classes = ["io", "blocking", "script"]
threads = 2
queue_capacity = 1024
max_inflight_bytes = 67108864

[core.execution_domains.lanes.background]
weight = 2
reserved_entries = 0
max_share_percent = 75
starvation_steps = 16
allow_idle_borrow = true
```

The `host.metrics` control method reports Core control/data mailbox pressure,
submit-to-dispatch, cancel, completion-route and scheduler latency counters,
plus queue/running/inflight capacity for every execution domain and lane.

The reference performance method is defined in MutsukiCore
`docs/issue43-acceptance.md`. It compares the same interactive task while an
identical blocking background task is active; only the execution topology
changes between the single-domain and multi-domain cases.
