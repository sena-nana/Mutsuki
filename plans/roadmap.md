# Mutsuki 路线图

Mutsuki 当前目标是 **极薄 single-task runtime + Plugin Runner** 的 Rust-first
runtime kernel。早期以 agent inbox、backend trait、多队列 runner inbox、Core 内置
broadcast / workflow 为中心的主链已经退出当前目标架构，也不作为公共兼容层保留。

## 当前边界

根级 workspace 由四个 crate 组成：

- `crates/mutsuki-runtime-contracts`：Task、Runner、Resource、Plugin load-plan、
  event/trace/error 等纯协议对象。
- `crates/mutsuki-runtime-core`：`CoreRuntime`、TaskStore/`TaskPool`、`TaskLease`、
  `RunnerRegistry`、Executor dispatch、`ResultRouter`、`StateStore`、
  `ResourceManager`、EventLog/TraceLog。
- `crates/mutsuki-runtime-host`：native runner host、deterministic load-plan resolver
  和 JSONL runner client。
- `crates/mutsuki-runtime-sdk`：Rust 插件作者侧 SDK，把 `TaskHandle` 包装为
  `Future`，提供 `ctx.call(...).await` 语法糖；不改变 Core 的 single-task 原语。

Python runner kit 位于 `python/mutsuki-runtime-python/`，镜像新协议并提供 Python
runner host、stdio JSONL runner server、ValueRef/ResourceRef 资源管理测试实现。

## 标准插件命名边界

第一批标准插件和协议包以 GitHub issue #8 为准：

- 插件分发包使用 `mutsuki-plugin-<domain>-<name>`。
- 协议包使用 `mutsuki-protocol-<domain>`。
- 标准插件运行时 ID 保留 `mutsuki.std.<domain>.<name>`。
- 协议 ID 使用 `mutsuki.<domain>.<action>`，不带 `plugin`。

当前保留的 QQ、LLM、Agent 周边只作为 postponed 或 dev-only 实验资产，不属于第一批
`mutsuki stdlib`，不得伪装为 `mutsuki.std.*` 标准插件。

## 已完成基线

- Rust contracts 覆盖 `Task`、`TaskStatus`、`ProtocolDescriptor`、`HandlerBinding`、
  `TaskLease`、`RunnerDescriptor`、`RunnerPurity`、`RunnerResult`、`StateDelta`、
  `EffectRequest`、`ResourceRef`、`ValueRef`、`ResourceCellRef`、`ResourceLease`、
  `RuntimeProfile`、`PluginManifest`、`RuntimeLoadPlan`、`ContractSurface`、
  `RuntimeEvent`、`TraceSpan`、`RuntimeError`。
- Rust core 覆盖：
  - `TaskPool` 统一保存 ready/running/completed/failed/cancelled task，并保留
    created/waiting/blocked/expired/dead_letter 结构化状态。
  - task 调度使用 `TaskLease`，一个 ready task 一次只会被一个 runner/executor
    lease 执行；`RunnerContext` 记录 executor id、task lease id、invocation id、
    cancel token 和 tick deadline。
  - `protocol_id` 是 task 调度事实源；旧 task kind 字段已从当前 wire shape 移除。
  - ready task claim 排序固定为 `ready_at_step asc -> priority desc ->
    created_sequence asc -> task_id asc`。
  - runner 按 protocol id、runner hint、purity claim task。
  - Runner loop 已预拆分为 `RunnerDispatch`、`RunnerExecutor` 和
    `RunnerCompletion` 边界；`CoreRuntime` 默认执行器仍是同步
    `InlineRunnerExecutor`，用于测试、replay 和最小 host；`HostRuntime`
    默认启动 CoreActor 控制线程并通过 mailbox / worker pool 执行普通 runner。
  - 调度策略第一阶段作为 host 级 `SchedulerPolicy` 外置；policy 只决定每个 runner
    本轮 dispatch budget，Core 继续拥有 TaskPool、TaskLease、状态迁移和最终校验。
    默认 scheduler 等价既有 runner limits / worker pool capacity 逻辑，并作为
    fallback 保留。
  - `RunnerDescriptor.execution_class` 标记 Control、Orchestration、Io、Cpu、
    Blocking、Script，host runtime 按 execution class 路由到普通或 blocking/script
    worker pool；`Control` 仅用于 core kernel 控制面。
  - `Runner` trait 要求 `Send`，native runner 可被 host worker 线程移动执行。
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
  - `ResourceManager` 支持 inline small value、ValueRef、blob ref、file-backed mmap
    ref、copy-on-write 和 ExclusiveWriteLease。
  - `ResourceManager` 的 descriptor、lease、occupancy 控制面已与当前本地
    `LocalResourceBackend` 读写实现分开；backend 仍是测试级 file/blob 实现，不是最终
    跨进程 provider。
  - `ResourceManager` 支持 `ResourceCellRef` / `ResourceLease`，长期资源状态归属
    ResourceManager，runner 只持有 step 期间的短期 lease。
  - `TaskHandle` / `TaskAwait` / `TaskOutcome` 是 SDK-facing 协议对象；Core 保存
    child task 到 parent continuation 的 wait link，但不引入 TaskGroup / WaitSet。
  - Runner 返回 `RunnerStatus::Waiting + TaskAwait` 时，Core 保存
    `continuation_ref`、释放当前 `TaskLease`，并在 child task 进入 terminal 状态后
    唤醒 parent task。
  - parent task 取消时，默认 `CancelPolicy::Cascade` 会取消正在等待的 child task；
    `Detach` / `Shield` 是协议预留，当前不扩展为 core workflow 语义。
  - task await 前会检查 ResourceManager 中当前 task 持有的短期可变 lease；存在
    exclusive write / exclusive ResourceLease 时 fail-loud，禁止跨 await。
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
    contract_fingerprint。
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
    `CommandPlan` 等协议对象。builtin 路径直接走本地 `CoreRuntime` /
    `ResourceManager`，ABI 路径通过 JSONL bridge 编码同一 wire shape。
  - native plugin host 可启动 `HostRuntime` 控制面门面，并通过
    `HostRuntimeCommand` / `HostRuntimeReply` 预留 CoreActor 消息边界；旧
    `into_runtime` 仍保留裸 `CoreRuntime` 兼容路径。
  - `HostRuntimeCommand::CancelTask` 先更新 Core task 状态；若 task 正在 worker
    内运行，CoreActor 记录 pending cancel，并在 worker 归还 runner 时通过
    `Runner.cancel(invocation_id)` 尽力投递。该路径不承诺抢占已卡死 native step。
  - `RunnerLimits.deadline_ticks` 接入 HostRuntime：host 为 dispatch 写入
    `RunnerContext.deadline_tick`，actor tick 发现超期 running invocation 后取消 Core
    task，并通过同一 runner management cancel 路径传播。
  - JSONL runner client 使用 `runner.step`、`runner.cancel`、`runner.dispose` 方法面。
- Python runner kit 覆盖：
  - 新协议 dataclass mirror 与 JSON roundtrip。
  - `PythonRunnerHost`、`StdioJsonlRunnerServer`、`PythonResourceManager`。
  - `RunnerContext` 镜像 invocation id、cancel token、deadline tick 与
    `cancel_requested`；`PythonRunnerHost.cancel_runner` 会记录 cancellation 并在后续
    step context 中传播，async runner context 暴露这些字段。
  - runner-side async adapter 将 `await ctx.call_raw(...)` 映射为 `RunnerStatus::Waiting`
    + child `Task` + `TaskAwait`，只驱动 Mutsuki task awaitable。
  - public API 不再导出早期 backend 兼容层。
  - Python 端仅保留当前 runner kit；旧 reference framework 已移除。
- Rust SDK 覆盖：
  - `RuntimeClient`、`TaskHandleFuture`、`SdkProtocol`、
    `AsyncRunnerContext::call::<P>` / `call_raw` / targeted variants。
  - `AsyncRunnerAdapter` 将 Rust `async` runner poll 为现有 sync `Runner.step` 结果。
  - 不依赖 Tokio / async-std；由 Core tick 和 event/outcome 查询驱动恢复。

## 当前完成门槛

当前 Rust/Python runtime framework 被视为可用，必须同时满足：

- `cargo fmt --check` 通过。
- `cargo test` 在根目录通过。
- `uv run ruff check src tests`、`uv run pyright src tests`、`uv run pytest` 在
  `python/mutsuki-runtime-python` 下通过。
- Core 不出现 Yume、LLM、IM、MCP、ChatCompletion 等业务执行分支。
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

- 加强真实跨进程 mmap/shared memory/blob provider，而不是当前测试级 file-backed mmap。
- 扩展 PluginHost resolver 的版本约束、权限审计和 capability provider 选择。
- 增加长期 sidecar supervision、独立 management channel / 强制隔离恢复和 effect compensation。
- 为 provider RPC 和 effect gateway 引入更完整的长期 supervision 与 compensation。
- 引入更完整的 executor supervision、TaskLease 过期回收和 stale executor commit fencing。
- 扩展 host runtime 的 worker health、wall-clock deadline、强制隔离恢复和更完整 drain supervision。
- 当确实出现多 host 策略、QoS、deadline、资源亲和性或租户隔离需求时，再把
  scheduler provider / plugin 化纳入 load-plan 设计；即便插件化，scheduler 也只能返回
  调度决策，不能执行 task、修改 TaskPool 或绕过 Core 生命周期。
- 扩展真实 Host backend registry / supervision；当前 `HostBackendDescriptor` 与
  `PluginBackend` 只固定边界和统一 client contract，不等于完成所有 backend 热替换执行器。

## 红线

- 不恢复旧实例私有收件队列作为核心事实源。
- 不引入 Runner 长期 inbox、Core 内置 broadcast、TaskGroup、WaitSet、Actor 或 Endpoint
  作为 core 一等 runtime 实体。
- 不把模拟个体、LLM、记忆、情感、IM 或产品 wire shape 写入 Rust core。
- 不让普通 runner 直接修改 StateStore/EventLog 或执行外部副作用。
- 不跨 ABI/进程传 Python object、Rust pointer、callable、socket、SDK client 或真实 handle。
