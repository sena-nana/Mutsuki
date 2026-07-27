---
name: distributed-recovery-performance
description: Validate Mutsuki distributed WAL, registry, compaction, replica receipts, checkpoint recovery, content localization, fault injection, and performance gates. Use for durability, recovery, or distributed performance changes.
---

# Distributed Recovery And Performance

- Prove WAL ordering, crash recovery, compaction and concurrent mutation linearization with behavior tests.
- Exercise process loss, duplicate delivery, stale receipt, partial write and corrupted record failures.
- Move blocking file and hash work off async reactors and measure both throughput and tail latency.
- Run `hosts/distributed/scripts/run-performance-model.py --mode smoke` for change validation.
- Preserve raw samples, anomaly analysis, repository revisions and environment fingerprint.
- Do not describe loopback or local multi-process evidence as real-network production performance.
