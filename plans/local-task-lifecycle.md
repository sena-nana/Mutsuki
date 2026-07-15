# 本地任务生命周期与低干扰观察契约

本文冻结单 Host Runtime 的任务 attempt、停止语义和观察边界。它只描述本地
`CoreRuntime` / `HostRuntime`，不引入节点、集群、全局任务、跨机租约或网络身份。

## Attempt 与 stale completion fencing

- 审计结果：当前仓库没有独立 `TaskFeedback` 或 `RunnerInstanceId` wire type；等价事实已经
  由 `EntryCompletion` / `CompletionBatch`、`TaskLease.runner_id`、`executor_id` 和 registry
  generation 表达。因此不新增重复协议对象，只补齐 TaskLease attempt 唯一性。
- 每个 task record 保存单调递增的 `attempt_generation`。每次 Ready task 被 claim 时
  generation 加一，并编码进既有 `TaskLease.lease_id`；不新增 runner wire 字段。
- completion 必须同时匹配 task 的 Running 状态、runner、executor、registry generation
  和完整 active `TaskLease`。校验与状态提交都在 Core actor 所有的 TaskPool 中完成。
- retry、lease expiry、reload rebind、deadline/cancel 和 Abort 都会释放或替换 active
  lease。旧 executor 即使在同一 step 返回，也必须以 `task.claim_conflict` 被拒绝，且
  不得写入状态、事件、资源、派生 task 或结果。
- `attempt_generation` 只用于本地诊断和 snapshot；它不是跨 Host/global lease。

## Cancel、Drain 与 Abort

| 操作 | 接受新 task | 已接收 task | 当前 invocation | 结果提交 |
|---|---|---|---|---|
| Cancel(task) | 不影响 | 指定 task 进入 Cancelled | 通过 runner management channel 协作取消 | 旧 lease 拒绝 |
| Drain | 拒绝新的外部 submit | 继续运行至 terminal | 正常完成 | active lease 可提交 |
| Abort | 永久拒绝 submit/tick/claim | 所有非 terminal task 原子取消 | Host 尽力投递 management cancel | 所有旧 lease 拒绝 |

Drain 只关闭外部接收面；已经接收的 task 及其正常派生 task 可继续推进，直到
`is_drained()` 为真。Abort 是不可逆的本地 runtime 状态。普通 host shutdown 先执行
Abort，确保运行环境失效后没有迟到结果可以回写。插件只看到既有 cancel token、deadline
和 runner management cancel，不看到 Drain/Abort 内部状态。

## Snapshot、事件与 correlation

- task snapshot 是稳定只读事实，包含 task 状态、active lease、registry generation、
  `attempt_generation`、trace/correlation 等 opaque descriptor。
- 生命周期低频事件为 `task.submitted`、`task.started`、`task.progress`、
  `task.completed`、`task.failed`、`task.cancelled`。`Continue` 不生成逐 tick progress。
- EventLog 与 TraceLog 是有界、非阻塞 outlet；每个 outlet 显式选择 drop-new 或
  drop-oldest，容量满时只增加 dropped counter，任务状态提交不等待消费者；容量设为 `0`
  会释放持久容器并完全关闭保留。
- 两类 outlet 都使用全局单调 sequence 和带 limit 的 `ObservabilityPage`；consumer 必须用
  `next_sequence` 继续，并检查 `lost` / `truncated`，不能把 retained 容器下标当稳定 cursor。
- scheduler decision 默认只更新累计计数。逐 decision event/span 与逐 dispatch span 必须
  在 RuntimeProfile 或 Host override 中显式开启；关闭时不构造保存用 trace attrs。
- 正确性、重试、停止与结果提交都不能依赖事件消费者或事件是否成功保留。

## 统计口径

`RuntimeStatistics` 公开当前状态计数和 TaskPool 在 actor 内维护的累计计数：submitted、
attempts started、queue steps、execution steps、stale results rejected；同时公开 retained / 
dropped events/traces 与 scheduler decisions。累计更新与任务状态迁移同路，常数成本且不启动采样线程。当前状态只在显式
查询 snapshot 时汇总。

本阶段不提供 P95、网络指标、采样器、后台线程或逐 tick 事件。时间口径是 deterministic
runtime step，不伪装为 wall-clock latency。

## 不变量

1. 同一 task 同时最多一个 active local attempt。
2. stop/lease 状态检查失败时，不能产生部分提交。
3. 事件 outlet 满或关闭时，任务仍能完成、失败或取消。
4. 所有 API 都保持单 Host 语义，不包含 distributed 专用类型或依赖。
