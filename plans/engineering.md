# Mutsuki 工程实现规则

根目录当前是 Rust-first 极薄 Tick-first / Batch-first runtime framework。Python runner kit 已拆分
到独立仓库；本仓库只保留 Rust core、contracts、host 和 Rust SDK。

## 1. 技术栈

- Rust 2024 + Cargo workspace。
- serde / serde_json 用于纯协议序列化。
- thiserror 用于 runtime failure wrapper。
- Python 3.13+ + uv 用于独立的 `MutsukiPythonRunnerKit` 仓库。

Rust crates 禁止依赖 Python、PyO3、产品协议 SDK、外部服务 provider 或领域语义。

## 2. 目录结构

```text
Mutsuki/
  Cargo.toml
  crates/
    mutsuki-runtime-contracts/  # Task / TaskLease / Runner / Resource / Plugin load-plan protocol
    mutsuki-runtime-core/       # CoreRuntime / TaskPool / TaskLease / Executor dispatch / ResourceManager
    mutsuki-runtime-host/       # runtime bootstrapper / JSONL runner client
    mutsuki-runtime-sdk/        # Rust SDK async/await wrapper over TaskHandle
    mutsuki-runtime-sdk-macros/ # Rust SDK proc-macro authoring DSL
  plans/
```

## 3. Crate 边界

- `mutsuki-runtime-contracts`：只定义纯数据结构，不包含 callable、socket、SDK client、
  真实 handle 或领域对象。
  `TransactionPlan`、`CommandBatch`、`SagaPlan` 和 `WorkflowDescriptor` 属于
  experimental descriptor：可作为 provider/workflow 边界 wire shape 保留，但默认不得被
  描述为 CoreRuntime 解释或执行的稳定语义。
- `mutsuki-runtime-core`：实现 TaskPool、TaskLease、RunnerRegistry、WorkBatch dispatch、
  ResultRouter、StateStore、ResourceManager、EventLog、TraceLog、hot-reload surface checks。
  Runner dispatch 可通过 `RunnerExecutor` 边界替换；core 默认只提供同步 inline
  执行器，不绑定线程模型。
- `mutsuki-runtime-host`：实现 native RuntimeBootstrapper/resolver、native runner wrapper 和
  stdio JSONL runner client，并提供默认 CoreActor / worker pool 隔离的
  `HostRuntime` 控制面门面。`RuntimeBootstrapper::into_runtime` 仍返回裸 `CoreRuntime`
  用于单线程测试、replay 和最小 host。Host backend / plugin backend 只能聚合统一
  `TaskClient` / `ResourcePlanClient` 和 host-side bridge/codec/service descriptor；不得把
  Host shell 整体做成插件。
- `mutsuki-runtime-sdk`：实现 Rust 插件作者侧 `RuntimeClient`、`TaskHandleFuture`、
  `AsyncRunnerContext` 和 `AsyncRunnerAdapter`；同时定义 host/plugin 扩展基础 trait：
  `PluginBuilder` / `PluginLoader`、`HostContext`、`HostServiceRegistry`、
  `CapabilityBroker`、`TaskSubmitter`、`ResourcePlanGateway`、`EventBridge`、
  `ConfigProvider` 和 `ShutdownController`。这些 trait 必须落回现有 contracts /
  host-side adapter，不得把 async runtime 语义或动态注册语义反向写入 Core。
- `mutsuki-runtime-sdk-macros`：只为 Rust 插件作者生成 `SdkProtocol`、
  `ResourceKind` / descriptor 和 async runner adapter glue；宏展开不得引入本地直调、
  workflow runtime、隐式调度或绕过 `TaskPool` 的执行路径。
- 外部 `MutsukiPythonRunnerKit`：镜像协议，提供 Python runner backend、stdio runner
  server、Python ResourceManager 测试实现、runner-side async adapter 和 typed public API；
  本仓库不包含该 Python 包源码。

## 4. 验证

根级 Rust 改动必须运行：

```powershell
cargo metadata --locked --format-version 1
cargo fmt --check
cargo test
bash scripts/check-distributed-boundary.sh
cargo bench-smoke
```

完整性能矩阵使用 `cargo bench-full`。scheduled/release CI 保存结构化结果，并对匹配历史 case
应用宽松相对阈值；普通 CI 的 smoke 只使用灾难性绝对上限，不把公共 runner 当微秒级稳定环境。

改动外部 Python backend kit 时，在 `MutsukiPythonRunnerKit` 仓库运行其 `uv` 验证命令。

不得用部分检查宣称成功。

## 5. 横切公约

- TaskPool 是 ready task backlog / 调度索引，不是 Runner inbox。
- TaskPool 增量索引必须能从 TaskRecord 确定性重建；所有状态迁移同步更新 ready、wake、
  expectation、lease expiry 和 runner load 索引。调度热路径不得使用 `records()` 快照、
  克隆全部候选 Task、重新排序完整候选集合或为 byte budget 重复 JSON 编码 payload。
- Task terminal history 上限只能显式启用；默认保留既有无限 outcome 语义。启用时 terminal
  record、payload byte cache、wait link 与防重 tombstone 必须一起保持有界，受 child wait
  保护的 record 不得提前淘汰，累计统计不能随 record 淘汰回退。
- Task 一次只能通过一个 TaskLease 交给一个 Runner / Executor 执行。
- Runner 是逻辑处理器，不是物理执行单元；Executor 是物理执行槽位。
- Rust `Runner` 必须是 `Send`；默认 host runtime 会把普通 runner 移动到 worker
  线程执行，CoreActor 不能直接执行插件 handler。
- `RunnerDescriptor.execution_class` 只用于 host 选择执行池，不改变 core 调度语义。
- Waiting task 释放 worker / lease，但继续占 runner 逻辑 inflight 配额。
- Runner inflight 只包含 Running 与 Waiting；Ready task 保留在 TaskPool backlog 中，
  不占用 inflight。Host `pool_queue_limit` 仅约束物理 worker dispatch 队列。
- HostRuntime cancel 先更新 Core task 状态，再通过 `Runner.cancel` 管理面投递。
  tick deadline 保持确定性取消语义；host-only wall-clock deadline、取消宽限和
  worker health timeout 可把卡死 native worker 隔离，但原线程退出前禁止补 replacement，
  达到隔离上限必须 degraded / 拒绝新 dispatch。process runner 可通过独立 termination
  handle kill 并重建；迟到 native completion 必须 drain / dispose，不能把旧结果或旧 runner
  重新放回 Core。
- 常驻 Host 应启用 event-driven driver；idle 时不得用固定 interval 调用 Tick。下一逻辑
  step 必须来自 TaskPool 增量索引与 running invocation deadline，timer 到期允许直接推进到
  目标 step。显式 tick 模式只用于 deterministic test、replay 和受控 embedding。
- 每次 claim 的 attempt generation 必须单调递增；Cancel、retry、reload、timeout 和 Abort
  之后，旧 TaskLease completion 必须原子拒绝。
- Drain 拒绝新的外部 submit 但允许已接收 task 完成；Abort 取消所有非 terminal task 并
  使 runtime 不可恢复。事件 outlet 必须有界、非阻塞且可关闭，正确性不得依赖消费者。
- runtime 累计统计只在 actor-owned 状态迁移上做常数成本更新；禁止为基础统计引入采样
  线程、逐 tick 事件、P95 或网络概念。
- EventLog/TraceLog 必须使用有界容器和单调 cursor；分页读取必须带 limit 并报告 lost /
  truncated。capacity=0 必须释放持久容器，trace 热路径必须在 attrs 构造前短路。
- scheduler decision 默认只做标量累计；逐 decision 明细和逐 dispatch span 必须由
  RuntimeProfile/Host 显式开启。
- 普通 runner 禁止直接副作用。
- StateStore 只能通过 `core.commit` task 修改。
- EventLog 只能通过 kernel event append 或 runtime 事件记录修改。
- Effectful runner 只处理 `effect.*` task。
- ResourceRef/ValueRef/ResourceCellRef/ResourceLease/StateRef 是跨边界 descriptor，不是语言对象引用。
- 长期资源状态归 ResourceManager / ResourceCell；runner 只能持有 step 期间的 ResourceLease。
- 具体资源数据读写归 backend / provider；ResourceManager 保留 descriptor、lease、
  occupancy 和 generation 事实源。
- 高级资源计划和 workflow 只能作为 provider/workflow plugin descriptor 暴露；
  CoreRuntime 不实现 transaction executor、command batch executor、Saga engine 或
  workflow runtime。
- Resource Provider 的运行中替换必须基于 `ResourceProviderReloadPolicy`、
  `ResourceProviderCompatibility` 和 live occupancy 判定；不能绕过 ResourceManager 的
  registry、lease、generation 规则。
- SDK 和 guest-side shim 是编译期/API 层，不是运行时插件；host-side shim / bridge 才能作为
  Host backend 替换。
- SDK 的 host/plugin 抽象只能用于 boot 前插件组装、host service 查询和统一 task/resource
  执行面；`HostServiceRegistry` freeze 后不得注册，`ConfigProvider` 不隐式读取环境变量或
  伪造默认值，`EventBridge` v1 只发布 outbound event。
- Core 不提供 TaskGroup、WaitSet、pipeline、broadcast、matcher、actor 或 endpoint runtime 实体。
- Rust SDK 可以提供 `ctx.call(...).await`，但其 wire 语义必须落到普通 task、
  `TaskAwait`、`Waiting`、wake 和 `TaskOutcome`。
- Runner capability 必须区分 `native_batch` 和 `scalar_adapter`；默认 capability 是
  scalar adapter 串行执行，`max_entry_concurrency = 1`、`preserve_order = true`、
  `side_effect = unknown`、`entry_cancel = false`，不能因为插件作者使用 scalar 写法而
  重新引入 single-task ABI。entry 并发声明不得超过 batch entry 上限。
- 当前 Runner instance model 是单实例：每个逻辑 `runner_id` 同时最多一个 active batch；
  `RunnerDescriptor.batch.max_inflight_batches` 和 Host `RunnerLimits.max_running` 必须为 `1`，
  大于或小于 `1` 都必须在启动阶段结构化拒绝。该限制不得误伤同一 batch 的多 entry 或
  entry-level parallelism。
- `WorkResourcePlan` 必须在 dispatch 前给出 parallel_groups、serial_groups 和
  parallelism_limit；Host/SDK adapter 只能在资源计划、runner capability、HostCapacity
  和 scheduler budget 均允许时并行执行 scalar entries。
- JS/TS SDK 不在当前 workspace；不得添加未接 runtime driver 的占位 API。Python
  runner-side awaitable adapter 位于独立 Python runner kit 仓库，不作为 Core 内置业务 SDK，
  也不承诺调度任意 `asyncio` future。
- registry boot 后 freeze；能力变化必须走新 registry generation。
- 错误必须结构化，不能吞异常返回默认值。
- ID、时间、随机源必须可注入或由 runtime/host 控制。
- Core、contracts、SDK 与普通 Host 的分布式零侵入边界按
  `plans/distributed-zero-intrusion-boundary.md` 执行；禁止多节点专用类型、依赖和
  `distributed` feature 分支进入插件编程模型。
- portable/checkpoint/content contracts 必须保持独立可选；不得把 attempt lease、部署位置、
  复制策略或恢复调度写入 descriptor。SDK `Checkpointable` 只能作为 side contract，不能
  修改 `Runner::run_batch`。未启用时不得执行哈希扫描、持久化 I/O 或启动后台线程。
- execution policy/variant/profile contracts 必须保持本地、可选和显式：禁止静默降质、隐式
  stale cache/partial result；画像只能使用固定容量窗口/直方图或标量，并由 Host 显式记录，
  不得接入每 tick 事件或后台采样。

## 6. Git 与范围

- 公共协议、core runtime、ResourceManager、RuntimeBootstrapper、热重载或目录边界变化，提交前必须检查 diff 范围。
- 不覆盖用户或其他 Agent 的已有改动。
