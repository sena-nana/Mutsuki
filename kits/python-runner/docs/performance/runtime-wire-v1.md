# Runtime Wire v1 性能基线

## Epic #30 P1 / schema 1.2

P1 镜像对应 MutsukiCore `180d47ea92747065a885aacfbcfd351d20d2170a`。连接先完成
Hello/Ack，再按协商后的总并发数和管理预留数开放业务请求；内部 runner link 只使用
length-prefixed typed MessagePack frame。阶段基准命令：

```text
uv run python benchmarks/run_p1.py full \
  --output artifacts/perf/issue30-p1-python-wire.json
```

报告约束取消 p95/max、16 与 56 并发吞吐不塌陷，以及峰值内存按并发请求摊销
不超过单请求两倍。性能产物记录 Python/Core revision 与 dirty 状态。

## Epic #30 P0 / schema 1.1

P0 镜像对应 MutsukiCore `94f84aca8ebb88b14d0067930557563e7f5d9cf3`、schema
`mutsuki.runtime.wire/1.1.0`。基准先预热 codec，并在逐次采样窗口暂停 GC 后取中位数；同机
控制组来自本仓库上一已推送 revision 与 schema 1.0。命令：

```text
uv run python benchmarks/run_p0.py \
  --baseline-ref d4ad12fb0a0a584391421568f32c037750a93ea6 \
  --output artifacts/perf/issue30-p0-python-wire.json
```

原始结果记录 binary codec 场景和门禁。门禁逐项约束 encode/decode p50 与 peak
allocation 不超过同机控制组的 1.5 倍，cancel p95 不超过 2 ms；最终结果必须为
`passed=true`。控制组通过同一基准脚本和上一已推送 revision 的独立导出运行，避免把历史机器
快照的调度噪声误判为代码退化。

## schema 1.0 历史基线

基线对应 MutsukiCore `d605333516753a797ca7e9971d097ac3e0de0c59`、schema
`mutsuki.runtime.wire/1.0.0`。测试机为 macOS / Python 3.14.3；命令：

```text
uv run python benchmarks/wire_benchmark.py
```

原始结果保存在 `artifacts/perf/issue2-runtime-wire.json`。时间为进程内 codec/dispatch
基线，不包含真实 pipe 调度、系统负载或业务 handler 时间；allocation 数据为 Python
`tracemalloc` peak bytes。

## Codec 结果

| 场景 | typed MessagePack encode/decode | frame bytes |
| --- | ---: | ---: |
| batch 1 × 1 KiB | 0.194 / 0.302 ms | 2,218 B |
| batch 32 × 1 KiB | 3.654 / 6.247 ms | 55,018 B |
| batch 256 × 1 KiB | 28.756 / 48.628 ms | 438,288 B |
| batch 1 × 64 KiB | 0.194 / 0.310 ms | 66,732 B |
| batch 1 × 1 MiB | 0.280 / 0.370 ms | 1,049,772 B |

当前 Python runner link 只保留 binary baseline。多 entry 小对象仍要按 workload
测量实际 handler/pipe 延迟；binary 的主要收益来自固定帧、安全 length check、较低资源
descriptor 开销和跨 host 的单一 codec 策略。

## Cancel、启动和资源

- 阻塞 `run_batch` 期间，经 concurrent binary reader/management dispatch 的 cancel：p50
  0.176 ms、p95 0.215 ms、max 1.400 ms（30 次，内存 stream）。
- Python 进程冷启动：p50 15.055 ms；已复用 runner 单 entry dispatch：p50 0.009 ms。
  生产路径应复用 runner process，不按请求启动进程。
- 1 MiB 资源只传 `ResourceRef`：JSON descriptor 441 B、MessagePack descriptor 360 B，
  分别为资源大小的 0.042% 和 0.034%；frame 不复制资源内容。

## 建议边界

- typed MessagePack 是正式 binary codec，适合较大但仍属普通 task 的结构化 payload；仍受
  4 MiB payload、8 MiB frame 和 64 in-flight 硬上限约束。
- 256-entry 已产生 16–49 ms codec 延迟，不建议作为低延迟默认值。高吞吐使用方应按 workload
  调整 batch，并保留 scheduler/deadline 预算。
- 大 bytes 不因 binary codec 而获准内联。超过 64 KiB 的资源内容必须使用 `ResourceRef`、
  stream 或 shared descriptor；1 MiB payload 场景只用于量化 codec，不是资源传输建议。
