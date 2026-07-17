# AgentKit Performance Model v1

该套件实现 MutsukiAgentKit #4，并作为 MutsukiCore #35 统一性能模型的业务层 owner
workload。`benchmarks/workloads-v1.json` 固定 fixture version、seed、case 与模拟延迟档。

## Fixture 与边界

- `mutsuki-agent-testkit::BenchmarkModelProvider` 和 benchmark tool 是测试专用公开 fixture，
  不作为生产 fallback。
- fake model/tool 支持 0 μs、1 ms、10 ms，不访问网络，输出由固定 seed 和输入决定。
- single/tool-chain/wait/session 通过真实 AgentLoop async continuation；tool route 通过真实
  ToolRegistry/ToolRouter。
- `agent.parallel-tools-8` 把 8 个纯工具放入真实 ToolRouter batch，并发执行 8 个 fixture
  target；报告同时保留 simulated wall time 与 8-tool work time。
- context 与 memory 使用真实 owner service；session-100 实际执行 100 次 get/model/append
  continuation，并记录 turn 10 后的 retained growth。

该 owner suite 不包含 Core scheduler 或 ServiceHost deployment，因此对应 overhead 明确为
0；不能拿它替代 Core/Host baseline。模拟 sleep 的调度超时会留在 Agent orchestration，
不会被隐藏为业务耗时。

## 运行与 repository revision snapshot

```text
python scripts/run-performance-model.py \
  --mode reference \
  --process-runs 3 \
  --repository MutsukiCore=../MutsukiCore \
  --repository MutsukiServiceHost=../MutsukiServiceHost \
  --repository MutsukiStdPlugins=../MutsukiStdPlugins \
  --output artifacts/performance/issue4-reference.json
```

输出包括每进程 raw samples、`mutsuki.performance.report/v1` 与 anomaly analysis。指标包括
p50/p95/p99/MAD、throughput、Agent orchestration、simulated wall/work、task、continuation、
tool route、allocation、CPU/RSS、retained memory 与 post-warmup growth。

## 正确性与异常归因

稳定 hash、重复 tool result、错误路由、意外错误和公网请求必须全部为 0。correctness
counter 非零时先核对 fixture/harness；只有排除测试实现问题后才标记 framework suspect。
正确性通过但 MAD 偏高时，仅作为环境或 case-specific noise，不直接归因框架。

本仓库在 `artifacts/performance/` 保留自己的 reference report、analysis、approval 与历史；
批准使用 MutsukiCore 的精确字节契约和本报告记录的 repository revision snapshot，不自动
接受新生成结果。
