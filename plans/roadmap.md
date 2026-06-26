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
    lease 执行；`RunnerContext` 记录 executor id 和 task lease id。
  - `protocol_id` 是 task 调度事实源；旧 task kind 字段已从当前 wire shape 移除。
  - ready task claim 排序固定为 `ready_at_step asc -> priority desc ->
    created_sequence asc -> task_id asc`。
  - runner 按 protocol id、runner hint、purity claim task。
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
  - JSONL runner client 使用 `runner.step`、`runner.cancel`、`runner.dispose` 方法面。
- Python runner kit 覆盖：
  - 新协议 dataclass mirror 与 JSON roundtrip。
  - `PythonRunnerHost`、`StdioJsonlRunnerServer`、`PythonResourceManager`。
  - public API 不再导出早期 backend 兼容层。
  - Python 端仅保留当前 runner kit；旧 reference framework 已移除。
- Rust SDK 覆盖：
  - `RuntimeClient`、`TaskHandleFuture`、`AsyncRunnerContext::call/call_targeted`。
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

## 下一步

- 加强真实跨进程 mmap/shared memory/blob provider，而不是当前测试级 file-backed mmap。
- 扩展 PluginHost resolver 的版本约束、权限审计和 capability provider 选择。
- 增加长期 sidecar supervision、deadline/cancel propagation 和 effect compensation。
- 为 provider RPC 和 effect gateway 引入更完整的长期 supervision 与 compensation。
- 引入更完整的 executor supervision、TaskLease 过期回收和 stale executor commit fencing。

## 红线

- 不恢复旧实例私有收件队列作为核心事实源。
- 不引入 Runner 长期 inbox、Core 内置 broadcast、TaskGroup、WaitSet、Actor 或 Endpoint
  作为 core 一等 runtime 实体。
- 不把模拟个体、LLM、记忆、情感、IM 或产品 wire shape 写入 Rust core。
- 不让普通 runner 直接修改 StateStore/EventLog 或执行外部副作用。
- 不跨 ABI/进程传 Python object、Rust pointer、callable、socket、SDK client 或真实 handle。
