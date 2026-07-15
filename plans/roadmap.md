# Mutsuki 路线图

Mutsuki 当前目标是 **极薄 Tick-first / Batch-first runtime + Plugin Runner** 的 Rust-first
runtime kernel。Core 只保留 TaskPool、RunnerRegistry、Runner dispatch、ResultRouter、
StateStore、ResourceManager、EventLog 和 TraceLog 等领域中立运行事实。

## 当前边界

根级 workspace 由四个 crate 组成：

- `crates/mutsuki-runtime-contracts`：Task、Runner、Resource、Plugin load-plan、
  event/trace/error 等纯协议对象。
- `crates/mutsuki-runtime-core`：`CoreRuntime`、TaskStore/`TaskPool`、`TaskLease`、
  `RunnerRegistry`、Executor dispatch、`ResultRouter`、`StateStore`、
  `ResourceManager`、EventLog/TraceLog。
- `crates/mutsuki-runtime-host`：runtime bootstrapper、deterministic load-plan resolver
  和 JSONL runner client。
- `crates/mutsuki-runtime-sdk`：Rust 插件作者侧 SDK，把 `TaskHandle` 包装为
  `Future`，提供 `ctx.call(...).await` 语法糖；单 task submit 由 SDK/Host 包装成
  one-entry batch。

Python runner kit 已拆分到独立 `MutsukiPythonRunnerKit` 仓库，镜像新协议并提供 Python
runner backend、stdio JSONL runner bridge 和测试替身。

## 标准插件命名边界

第一批标准插件和协议包以 GitHub issue #8 为准：

- 插件分发包使用 `mutsuki-plugin-<domain>-<name>`。
- 协议包使用 `mutsuki-protocol-<domain>`。
- 标准插件运行时 ID 保留 `mutsuki.std.<domain>.<name>`。
- 协议 ID 使用 `mutsuki.<domain>.<action>`，不带 `plugin`。

标准协议与插件已迁移到独立 `MutsukiStdPlugins` 仓库，包括
`mutsuki-plugin-resource-memory`、shared-memory provider、`mutsuki-plugin-dev-mock`、
`mutsuki-plugin-observe-log`、`mutsuki-plugin-config-permission`、
`mutsuki-plugin-workflow-linear`、`mutsuki-plugin-workflow-broadcast`、
`mutsuki-plugin-io-fs`、`mutsuki-plugin-io-http-client` 和 `mutsuki-plugin-db-sqlite`
能力。`dev-mock` 当前提供 deterministic echo / sleep / fail / random-fail /
resource lineage mock，用于验证标准插件加载、协议声明、handler binding 和 batch runner
路径；`observe-log` 当前提供 `mutsuki.log.emit` 与最小 trace event 协议到 DomainEvent 的
标准化映射；`config-permission` 当前提供 `mutsuki.config.describe` 与 payload-driven
`mutsuki.permission.check`，用于验证配置/权限类协议可以作为普通插件通过 load plan 接入。
workflow 插件当前通过普通派生 task 表达 linear strict sequence 与 explicit target
broadcast；fs / http / sqlite 插件通过 public facade task 派生 `effect.*` task 执行真实副作用，
保持 Core 只调度协议事实。后续能力继续作为普通插件 / 协议包实现，不能回灌为 Core 内置业务协议、
外部副作用或应用状态分支。

`MutsukiStdPlugins` 当前包含 `mutsuki-protocol-workflow`、`mutsuki-protocol-resource`、
`mutsuki-protocol-fs`、`mutsuki-protocol-http`、`mutsuki-protocol-db`、
`mutsuki-protocol-observe`、`mutsuki-protocol-config` 和 `mutsuki-protocol-dev`。
这些协议包只暴露 protocol id 常量、`VERSION`、`ABI_CODEC`、稳定 protocol id 列表和
input / output / error schema 函数，不提供 runner、executor、资源后端或默认执行策略。
SDK helper types 与更细粒度 compatibility rules 后续在协议 wire shape 细化时继续补齐。

## 已完成基线

- Rust contracts 覆盖 `Task`、`TaskStatus`、`ProtocolDescriptor`、`HandlerBinding`、
  `TaskLease`、`RunnerDescriptor`、`RunnerPurity`、`RunnerResult`、`StateDelta`、
  `EffectRequest`、`ResourceRef`、`ValueRef`、`ResourceCellRef`、`ResourceLease`、
  `RuntimeProfile`、`PluginManifest`、`RuntimeLoadPlan`、`ContractSurface`、
  `RuntimeEvent`、`TraceSpan`、`RuntimeError`。
- Rust core 覆盖：
  - `TaskPool` 统一保存 ready/running/completed/failed/cancelled task，并保留
    created/waiting/blocked/expired/dead_letter 结构化状态。
  - 每次 claim 递增本地 attempt generation 并生成唯一 TaskLease id；retry、reload、
    timeout、Cancel 和 Abort 后的 stale completion 都会被 active lease fencing 拒绝。
  - `CoreRuntime` / `HostRuntime` 已区分 Cancel、Drain 和 Abort；Drain 停止外部接收但允许
    已接收任务完成，Abort 取消非 terminal task 并使旧 attempt 失效。
  - EventLog/TraceLog 是可配置容量与 drop-oldest/drop-new 策略的非阻塞 outlet，容量为零
    释放持久容器；两者通过单调 sequence 的有界 page 报告 cursor 丢失和截断。scheduler
    decision 默认聚合计数，逐 decision/dispatch trace 由 profile 显式开启；观察统计不参与
    任务正确性。
  - task 调度使用 `TaskLease`，一个 ready task 一次只会被一个 runner/executor
    lease 执行；`RunnerContext` 记录 executor id、task lease id、invocation id、
    cancel token 和 tick deadline。
  - TaskPool 由权威 TaskRecord 增量维护 protocol/hint/owner 复合 ready queue、wake/lease
    到期桶、expected-version ready set 和 runner running/waiting 集合；claim 只克隆最终入选
    Task，payload compact JSON 字节数在 enqueue 时缓存，普通 tick 不再按 runner 扫描全表或
    重复序列化预算。
  - `protocol_id` 是 task 调度事实源；当前 wire shape 不包含 task kind 兼容字段。
  - ready task claim 排序固定为 `ready_at_step asc -> priority desc ->
    created_sequence asc -> task_id asc`。
  - runner 按 protocol id、runner hint、purity claim task。
  - Runner loop 已预拆分为 `RunnerDispatch`、`RunnerExecutor` 和
    `RunnerCompletion` 边界；`CoreRuntime` 默认执行器仍是同步
    `InlineRunnerExecutor`，用于测试、replay 和最小 host；`HostRuntime`
    默认启动 CoreActor 控制线程并通过 mailbox / worker pool 执行普通 runner。
  - 调度策略第一阶段作为 host 级 `SchedulerPolicy` 外置；policy 只决定每个 runner
    本轮 dispatch budget，Core 继续拥有 TaskPool、TaskLease、状态迁移和最终校验。
    默认 scheduler 等价既有 runner limits / worker pool capacity 逻辑；policy
    决策失败必须结构化失败，不再 fallback。
  - `RunnerDescriptor.execution_class` 标记 Control、Orchestration、Io、Cpu、
    Blocking、Script，host runtime 按 execution class 路由到普通或 blocking/script
    worker pool；`Control` 仅用于 core kernel 控制面。
  - `Runner` trait 要求 `Send`，native runner 可被 host worker 线程移动执行。
  - 当前明确采用单实例串行 Runner：每个 `runner_id` 同时最多一个 active batch，
    descriptor 的 `max_inflight_batches` 与 Host `max_running` 只接受 `1`；batch 内仍可按
    entry capability 和资源计划有界并行，不同 runner 可并行执行。
  - Task terminal history 默认保持既有无限保留语义；长期 Host 可显式配置
    `TaskHistoryRetention`，把 terminal TaskRecord 与已淘汰 task id 防重窗口分别限制为固定
    容量。受 child wait link 保护的 terminal record 在 waiter 消费前不会淘汰；累计 submitted、
    attempt 和 evicted 统计不因淘汰回退。
  - Waiting task 释放当前 `TaskLease`，但保留 `owner_runner`；wake 后的 continuation
    只能由原 runner reclaim，并计入 runner waiting / inflight 负载统计。
  - `HandlerBindingRegistry` 从 load plan 物化，提供 protocol 到逻辑消费者绑定的
    查询索引；Core 不自动 fan-out。targeted task 由编排插件或 thin core facade
    显式创建。
  - 复杂编排、顺序执行、广播、pre/filter/post、matcher 均由普通插件 runner 通过
    `RunnerResult.tasks` 或监听事件后显式创建后续 task。
  - Pure runner 输出的 state delta、event、effect request 被 ResultRouter 转为
    `core.commit`、`core.event.append`、`effect.*` task。
  - `core.kernel` Committer runner 是 StateStore/EventLog 的提交入口。
  - `ResourceManager` 支持 inline small value、ValueRef、provider-backed
    `ResourceRef` descriptor 和 ExclusiveWriteLease fact。
  - `ResourceManager` 的 descriptor、lease、occupancy 控制面已与 resource provider
    数据面分开；core 测试和 load-plan fixture 显式使用
    `mutsuki.std.resource.memory` provider surface，Rust core 不再包含兼容测试级
    file/blob/mmap store 或默认 provider 配置。
  - `CoreRuntime` 公开 task facade 已收敛为 `TaskHandle` 入口，字符串 task id 只作为
    TaskPool、cascade cancel 和 host actor bookkeeping 的内部事实键。
  - `Runner.run_batch` 已成为 runner 标准入口；host 在同一 tick 按 scheduler budget
    claim 多个 task，构造 `WorkSet -> WorkResourcePlan -> WorkBatch` 后派发。
  - `CoreRuntime` 不再暴露 resource plan 数据面执行 facade，也不再暴露 blob/mmap/COW
    创建、原始 resource read 或 bytes write facade；collect/snapshot、
    export、command/batch/saga 等 plan 执行和资源 bytes 创建/读取/写入必须经 host resource backend /
    `ResourceProviderGateway`。当前 `ResourceManager` 只保留 plan 构造、descriptor
    路由和 lease / occupancy 事实。
  - Provider 执行 write commit、command、batch、saga 后通过 `PlanReceipt.descriptor_updates`
    回写新的 `ResourceRef` descriptor；host actor 只将 descriptor/generation 同步进
    `ResourceManager`，不把资源 bytes 数据面拉回 Core。
  - `ResourceManager` 支持 `ResourceCellRef` / `ResourceLease`，长期资源状态归属
    ResourceManager，runner 只持有 step 期间的短期 lease。
  - `TaskHandle` / `TaskAwait` / `TaskOutcome` 是 SDK-facing 协议对象；Core 保存
    child task 到 parent continuation 的 wait link，但不引入 TaskGroup / WaitSet。
  - Runner 返回 `RunnerStatus::Waiting + TaskAwait` 时，Core 保存
    `continuation_ref`、释放当前 `TaskLease`，并在 child task 进入 terminal 状态后
    唤醒 parent task。
  - `RunnerDescriptor.batch.mode` 区分 `native_batch` 与 `scalar_adapter`；默认 capability
    是 scalar adapter，entry 并发为 1，默认保序、不默认 entry-level cancel、未知副作用，
    Host/Core 仍通过 WorkBatch / CompletionBatch 统一执行与校验。
  - `WorkResourcePlan` 暴露 parallel_groups、serial_groups 和 parallelism_limit，供
    Host/SDK scalar adapter 在资源计划能证明安全时有界并行，否则保持串行。
  - parent task 取消时，默认 `CancelPolicy::Cascade` 会取消正在等待的 child task；
    `Detach` / `Shield` 是协议预留，当前不扩展为 core workflow 语义。
  - task await 前会检查 ResourceManager 中当前 task 持有的短期可变 lease；存在
    exclusive write / exclusive ResourceLease 时 fail-loud，禁止跨 await。
  - 可选 `PortableTask` / `TaskCheckpoint` / `ContentId` / `PortabilityCatalog` 支持单机录制、
    流式重放、checkpoint 恢复和内容寻址；恢复仍创建普通本地 Task/ResourceRef，默认
    LocalOnly 路径不增加后台线程、哈希或持久化开销。
  - 可选 `ExecutionPolicy` / capability requirement / variant catalog 支持单机 CPU、CUDA、
    Metal、Vulkan 等实现的显式选择与降质报告；固定 32 项窗口、固定直方图和 EWMA 由 Host
    显式更新，未启用时不接入 Core 热路径。
  - registry boot 后 freeze；runner descriptor 必须在 `RuntimeLoadPlan` 授权内。
  - hot reload 支持 contract surface 比较：Identical、Additive、Deprecated、
    Removed、Breaking；breaking 会阻断。
  - `reload_with_runners` 物化新 runner generation，先以 shadow registry 校验，再
    切换 active generation。
  - ready task rebind 到新 registry generation；clean / local dirty running
    invocation 通过原 runner management cancel 回到 ready；polluted / unknown
    invocation 保留旧 registry 进入 draining，settle 后再 dispose。
  - TaskPool / ResourceManager 提供 live surface occupancy，用于 removed/deprecated
    surface 安全检查；effect in-flight、stream、subscription 和 timer 均进入显式
    contract surface。
  - runner cancel 走 management channel，dispose 进入 DisposeBag；TraceLog 记录
    runner_id、plugin_id、plugin_generation、artifact_hash、descriptor_hash 和
    contract_fingerprint，并将 runner.run_batch span 绑定到 batch id、tick id、
    entry ids、task lease ids、executor id 和 correlation id。
- Rust host 覆盖：
  - native plugin host 可解析 `RuntimeProfile + PluginManifest` 为 load plan 并启动
    `CoreRuntime`。
  - resolver 支持 `RuntimeProfile.mode`：`FullDev` / `ExtensibleRuntime` 保守保留
    manifest 声明的系统扩展，`BuiltinOnly` / `LockedBuiltin` 按 enabled plugins、
    deployment 与 manifest `requires` 生成 `RuntimeCapabilityGraph`，并在 Level 1
    裁剪未激活的 host backend、plugin backend、codec、bridge、scheduler policy
    和 workflow surface。
  - Host 提供 `LocalTaskClient` / `AbiTaskClient` 与
    `LocalResourceClient` / `AbiResourceClient` 两组后端；它们接收同一套
    `Task`、`TaskHandle`、`TaskOutcome`、`ReadPlan`、`WritePlan`、`ExportPlan`、
    `CommandPlan` 等协议对象。builtin 资源路径现在通过可注入的
    `ResourceProviderGateway` 执行资源创建和 plan 数据面；HostRuntimeCommand
    没有 provider 时会结构化失败，不能回退到 core-managed store；ABI 路径通过
    JSONL bridge 编码同一 wire shape。
  - native plugin host 可启动 `HostRuntime` 控制面门面，并通过
    `HostRuntimeCommand` / `HostRuntimeReply` 预留 CoreActor 消息边界；
    `into_runtime` 保留裸 `CoreRuntime` 路径用于单线程测试、replay 和最小 host。
  - HostRuntime 可显式启用 event-driven driver：submit、worker completion、cancel、
    reload 和 shutdown 直接通过 actor mailbox 唤醒；TaskPool 用增量 step index 报告
    `next_required_step`，actor 只为最近 ready/wake/tick deadline 或 host wall-clock
    supervision deadline 安排一次性等待。无任务、无 deadline 时 actor 永久阻塞，不产生
    空 Tick；显式 tick 模式继续用于 deterministic test、replay 和 embedding。
  - `HostRuntimeCommand::CancelTask` 先更新 Core task 状态；若 task 正在 worker
    内运行，CoreActor 记录 pending cancel，并在 worker 归还 runner 时通过
    `Runner.cancel(invocation_id)` 尽力投递。该路径不承诺抢占已卡死 native step。
  - `RunnerLimits.deadline_ticks` 接入 HostRuntime：host 为 dispatch 写入
    `RunnerContext.deadline_tick`，actor tick 发现超期 running invocation 后取消 Core
    task，并通过同一 runner management cancel 路径传播。
  - HostRuntime 支持 host-only wall-clock deadline、取消宽限和 worker health timeout。
    超时或取消后仍不归还的 native worker 会被隔离，blocking/script pool 会补充
    replacement worker；迟到 completion 进入 drain，只投递 management cancel /
    dispose，不再把旧 runner 或结果放回 Core。
  - Host scheduler 的 `HostCapacity` 暴露 running/queued batch 与 entry 数、saturation、
    preferred batch size、max entry concurrency 和 max inflight bytes；SchedulerPolicy
    只读这些事实并返回 dispatch budget。
  - JSONL runner client 使用 `runner.run_batch`、`runner.cancel`、`runner.dispose` 方法面。
  - Host backend / plugin backend 裁剪已覆盖 Level 2 boot gate：resolver 生成 active
    capability graph 和 contract surface，RuntimeBootstrapper / host boot 按
    `RuntimeCapabilityGraph.active_*` 注册或拒绝 plugin backend、bridge、codec 与
    scheduler policy。active backend 的 bridge deployment 与 codec 引用在启动前校验；
    配置的 host scheduler policy 必须匹配 active scheduler descriptor。完整长期
    supervision / replacement registry 仍属于后续工作。
  - `HostRuntime` 暴露 SDK-facing `HostContext`，将 active capability graph、
    host-only service registry、config/event bridge、task submitter、resource backend
    和 shutdown controller 组合为 host 扩展基础设施；这些接口复用既有
    `HostRuntimeCommand`、`TaskClient`、`ResourcePlanClient` 与 load-plan descriptor，
    不新增 Core 运行时事实。
- 外部 Python runner kit 覆盖：
  - 新协议 dataclass mirror 与 JSON roundtrip。
  - `PythonRunnerBackend`、`StdioJsonlBridge`、`PythonResourceManager`。
  - `RunnerContext` 镜像 invocation id、cancel token、deadline tick 与
    `cancel_requested`；`PythonRunnerBackend.cancel_runner` 会记录 cancellation 并在后续
    step context 中传播，async runner context 暴露这些字段。
  - runner-side async adapter 将 `await ctx.call_raw(...)` 映射为 `RunnerStatus::Waiting`
    + child `Task` + `TaskAwait`，只驱动 Mutsuki task awaitable。
  - public API 只面向 runner、resource descriptor 和 JSONL runner server。
  - Python 端仅保留 runner kit，不进入本 Rust workspace。
- Rust SDK 覆盖：
  - `RuntimeClient`、`TaskHandleFuture`、`SdkProtocol`、
    `AsyncRunnerContext::call::<P>` / `call_raw` / targeted variants。
  - `AsyncRunnerAdapter` 将 Rust `async` runner 作为 one-entry scalar adapter poll 为
    `CompletionBatch` entry result。
  - `ProtocolSpec`、`ResourceKindSpec`、descriptor builders 和
    `mutsuki-runtime-sdk-macros` 提供 Rust 插件作者侧 typed DSL，用于生成
    protocol/resource/runner descriptor 与 `AsyncRunnerAdapter` glue；宏只展开为现有
    Task / RunnerResult / manifest 声明，不新增 Core 运行时语义。
  - 不依赖 Tokio / async-std；由 Core tick 和 event/outcome 查询驱动恢复。
  - 核心 SDK 公共面已收敛为 `ResourceKind`、`TypedResourceHandle`、`ResourceClient`
    和 plan 构造；`TextBuffer`、`AstSnapshot`、`ProjectFacts`、`ModelOutputStream`、
    `DbPool` 这类 resource descriptor marker 只保留在示例 / 测试 helper 或领域包中。
  - SDK 现在提供 `plugin` / `host` / `backend` 基础抽象：`PluginBuilder` /
    `PluginLoader` 只构造 boot 前 manifest、runner 与 host service 声明；
    `HostContext`、`CapabilityBroker`、`TaskSubmitter`、`ResourcePlanGateway`、
    `EventBridge`、`ConfigProvider` 和 `ShutdownController` 只包装现有协议对象和
    host-side 执行面，不允许运行中动态越权注册。

## 当前完成门槛

当前 Rust runtime framework 被视为可用，必须同时满足：

- `cargo fmt --check` 通过。
- `cargo test` 在根目录通过。
- Core 不出现业务协议、产品集成、外部服务或应用状态执行分支。
- TaskPool 只承载控制面和引用，不搬运不可控业务对象。
- TaskPool 不是 Runner inbox；Runner 不长期持有 Task 权威状态。
- 状态只通过 `core.commit` task 提交；外部副作用只通过 `effect.*` task 处理。
- ResourceRef/ValueRef/ResourceCellRef/ResourceLease 是 descriptor，不是语言对象或裸指针。
  - Plugin/runtime registry 由 load plan 物化，运行中不得动态越权注册能力。
  - Core 不提供 TaskGroup、WaitSet、pipeline.run、broadcast.run、matcher.run 或 actor.send。
  - Core 不暴露 Rust `Future` ABI；async/await 只属于 SDK / protocol helper 层。
  - Host / Plugin backend、codec、bridge、scheduler policy 和 workflow 以 descriptor
    进入 load-plan surface；Host shell、Core kernel、SDK 和 guest-side shim 不作为运行时
    热替换插件。
  - Resource Provider 热替换规则由 `ResourceProviderReloadPolicy` 与
    `ResourceProviderCompatibility` 显式描述；Resource Registry、ResourceId 和 lease
    基础规则仍归 Core / ResourceManager。

## 下一步

- 在 `MutsukiStdPlugins` 继续加强 `mutsuki-plugin-resource-memory` provider；host 侧
  `ResourceProviderGateway` 注入边界已覆盖资源创建、plan 执行和 provider commit 后
  descriptor registry 同步，core 只保留 descriptor、lease、generation 与 occupancy 事实。
- 加强真实跨进程 mmap/shared memory/blob provider，而不是当前测试级 file-backed mmap。
- 扩展 RuntimeBootstrapper resolver 的版本约束、权限审计和 capability provider 选择。
- 增加长期 sidecar supervision、独立 management channel / 强制隔离恢复和 effect compensation。
- 为 provider RPC 和 effect gateway 引入更完整的长期 supervision 与 compensation。
- 在既有本地 attempt generation、TaskLease 过期回收和 stale executor commit fencing 上
  扩展更完整的 executor supervision。
- 扩展跨进程 runner / sidecar 的独立 management channel、强制终止和补偿语义；当前
  HostRuntime 已覆盖 native worker health、wall-clock deadline、隔离 replacement 和
  迟到 completion drain。
- 当确实出现多 host 策略、QoS、deadline、资源亲和性或租户隔离需求时，再把
  scheduler provider / plugin 化纳入 load-plan 设计；即便插件化，scheduler 也只能返回
  调度决策，不能执行 task、修改 TaskPool 或绕过 Core 生命周期。
- 扩展真实 Host backend supervision；当前 `HostExtensionDescriptor` 与
  `PluginBackend` 固定边界和统一 client contract，Host 启动已按
  `RuntimeCapabilityGraph.active_*` 裁剪 plugin backend、bridge、codec 与 scheduler
  policy。后续重点是 backend / bridge / codec 的长期连接监督、drain、replacement 与
  workflow 实例状态资源化。

## 红线

- 不引入实例私有收件队列作为核心事实源。
- 不引入 Runner 长期 inbox、Core 内置 broadcast、TaskGroup、WaitSet、Actor 或 Endpoint
  作为 core 一等 runtime 实体。
- 不把业务对象、产品协议或应用 wire shape 写入 Rust core。
- 不让普通 runner 直接修改 StateStore/EventLog 或执行外部副作用。
- 不跨 ABI/进程传 Python object、Rust pointer、callable、socket、SDK client 或真实 handle。
- 不把节点、集群、Leader/Follower、quorum、全局租约、跨机 transport、远程资源位置或
  trust 语义写入 Core、通用 contracts、SDK 或普通 Host；完整规则见
  [distributed-zero-intrusion-boundary.md](distributed-zero-intrusion-boundary.md)。
