# QQ AI pipeline performance acceptance (Issue #140)

Issue #140 uses `benchmarks/workloads-v2.json` as the versioned workload and
`scripts/run-performance-model.py` as the executable acceptance gate. The report separates Bot
owner orchestration time from deterministic platform delay and records the workspace revision,
dirty state, toolchain, CPU, memory, sampling configuration, raw samples, and output hashes.

The reference gate requires all declared budgets to be evaluated and pass:

| Workload | Acceptance |
| --- | --- |
| `bot.flow-chain-single/100/10k` | 10k typed three-node chain p95 <= 500 ms |
| `bot.flow-fanout-64x10k` | 640k explicit child tasks p95 <= 10 s |
| `bot.flow-node-wait-resume` | node child-task wait/resume absolute budget |
| `bot.conversation-session-1k` | p95 orchestration <= 3 ms |
| `bot.delivery-idempotency-1k` | p95 <= 4 ms and median >= 300k events/s |
| `bot.interaction-transition-1k` | p95 <= 12 ms and median >= 100k events/s |
| `bot.duplicate-event` | zero duplicate executions |
| `bot.long-run` | p95 orchestration <= 100 ms |

Run the reference gate from `plugins/bot`:

```text
python3 scripts/run-performance-model.py \
  --mode reference \
  --process-runs 3 \
  --output artifacts/performance/issue140-reference.json
```

The benchmark intentionally performs no public QQ or Agent network request. Real-account QQ
latency and platform quota behavior remain an external smoke boundary and are not inferred from
the loopback results. Functional acceptance publishes an immutable graph and uses the real
ServiceRuntime path in `mutsuki-bot-service-host-integration/tests/qq_ai_pipeline.rs`.
