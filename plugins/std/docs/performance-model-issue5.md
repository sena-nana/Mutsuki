# Standard Plugins Performance Model v1

该套件实现 MutsukiStdPlugins #5，并作为 MutsukiCore #35 统一性能模型的 owner
基准之一。fixture manifest 固定在 `benchmarks/workloads-v1.json`，seed 为
`1297435713`。

## 测量边界

- workflow linear/broadcast：真实 Runner，规模 1/8/64。
- memory：1 MiB smoke；reference 增加 64 MiB，分别覆盖 shared read 与 COW commit。
- shared memory：真实 descriptor/open/read/release，校验 mapped view copied bytes 为 0。
- SQLite：临时数据库、固定 64 行、真实 query effect handler。
- filesystem：临时目录树、固定 8 文件、真实 batched read effect handler。
- HTTP：只允许确定性 loopback server；不产生公网请求。
- observe：真实 64-entry batch；permission 与 dev mock 使用真实公开 Runner。

每个 case 的计时边界只包含 plugin handler/provider 调用，Core 调度和 ServiceHost
部署成本不混入业务插件增量。allocation 按每个 measured batch 归一化；HTTP case 的
allocation 包含同进程 loopback fixture server，并通过 case 边界保留这一限制。系统 case
单独记录 benchmark child process 的 CPU 与 peak RSS，不包含 cargo build。Windows 使用
Win32 process counters，Unix 使用单进程 `wait4` resource usage。

## 运行

```text
python scripts/run-performance-model.py \
  --mode reference \
  --process-runs 3 \
  --repository MutsukiCore=../MutsukiCore \
  --repository MutsukiServiceHost=../MutsukiServiceHost \
  --output artifacts/performance/issue5-reference.json
```

输出同时生成 `*-raw/`、统一 v1 report 与 `*-analysis.json`。reference 默认每个
process 30 samples、3 process runs；smoke 默认 3 samples、1 process run。

## 正确性与异常判定

跨样本和跨进程必须保持 output hash 一致，且 `runner_errors`、
`public_network_requests`、`cross_process_hash_mismatches` 均为 0。任何 correctness
counter 非零时分类为 `framework-suspect`，必须先判断 fixture/harness 是否错误；正确性
通过后，高 MAD 仅分类为环境或 case-specific noise，不能直接归因于框架。

本报告不能代替 Core 或 ServiceHost 基准，也不声称 loopback HTTP 等同于公网性能。
本仓库在 `artifacts/performance/` 保留自己的 macOS ARM64/Windows x64 report、analysis、
approval 与历史；批准使用 MutsukiCore 的精确字节契约，不自动接受新生成结果。
