# Mutsuki 架构设计

Mutsuki 的当前根级实现收敛为领域中立的 **极薄 single-task runtime**。

核心定位：

```text
Core = TaskStore / TaskPool + TaskLease + RunnerRegistry + Executor dispatch
     + ResultRouter + StateStore + ResourceManager + EventLog + TraceLog
```

Core 只保证单个 Task 的状态、入池、租约、执行提交和资源引用一致性。复杂流程、
顺序执行、广播、pre/filter/post、matcher 分派、IM/HTTP/日志 pipeline 均由协议包
或编排插件在 Core 之上实现。

## 1. 分层

```text
RuntimeProfile + PluginManifest
  -> external/native resolver
  -> RuntimeLoadPlan / RuntimeLock
  -> CoreRuntime boot
  -> protocol task submit
  -> TaskStore / TaskPool
  -> TaskLease
  -> Executor invokes Runner.step
  -> ResultRouter / StateStore / ResourceManager
```

依赖方向：

- `contracts` 只定义 serde 纯协议对象。
- `core` 依赖 `contracts`，只实现 runtime mechanics。
- `host` 依赖 `core + contracts`，提供 native runner host 和 JSONL runner client。
- Python runner kit 镜像 contracts，提供 Python runner host 和 stdio runner server；
  Rust crates 不依赖 Python。

## 2. Protocol

`Protocol` 只表示数据契约：

- `protocol_id`
- input / output / error schema
- version
- ABI-safe codec
- compatibility rules

Protocol 不定义默认 pool、默认 task type、single/broadcast、pre/post、runner 选择、
overflow policy 或业务 workflow。

## 3. HandlerBinding

`HandlerBinding` 是插件声明自己可以处理某个 protocol 的逻辑消费绑定。它说明：

- 哪个 plugin 可以消费哪个 protocol。
- 该绑定目标 protocol / runner hint / pool。
- 可选 priority / policy / metadata。

编排器查询 `HandlerBindingRegistry`，再显式创建 targeted task。Core 不因为新增
binding 自动 fan-out，也不把一个 Task 交给多个 runner。

## 4. Task 与 TaskPool

`Task` 是 Core 中唯一可调度对象，是某次 protocol 处理请求生成的唯一状态机实例。

Task 具有：

- `task_id`
- `protocol_id`
- `input_refs`
- `output_ref`
- `continuation_ref`
- `target_binding_id`
- `lease_id`
- `trace_id` / `correlation_id`
- `registry_generation`

`TaskPool` 是 ready task backlog / 调度索引，不是 Runner inbox。没有 runner 时，
Task 仍然可以保持 Ready 并积压。Runner 不长期拥有 Task。

Core 保留单 Task 状态：

```text
Created / Ready / Running / Waiting / Blocked / Completed / Failed / Cancelled / Expired / DeadLetter
```

`Waiting` 和 `Blocked` 只描述当前 Task 自身状态，不表示等待一组 task、workflow
stage 或 broadcast completion。

## 5. TaskLease / Executor / Runner

Core 的唯一执行语义：

```text
一个 Task -> 一个 TaskLease -> 一个 Executor 调用一个 Runner
```

`Runner` 是插件提供的逻辑处理器，负责推进一个 task step 并返回结果。Runner 不持有
Task 权威状态、workflow 状态、连接池、长期 stream、lock 或 transaction。

`Executor` 是物理执行槽位，可以是线程、进程、Wasm instance、沙箱、远程 worker 或
插件运行实例。Executor 可销毁；销毁不应导致 Task 状态、Continuation、连接池或资源
状态丢失。

当前 Rust core 的 `Runner.step` 仍使用 `Vec<Task>` wire shape 以保持 host/JSONL
兼容，但调度器每次只租出一个 task 给一个 executor。

## 6. ResultRouter

RunnerResult 不直接修改事实源：

- completed / failed / cancelled / waiting / blocked 更新当前 Task。
- `deltas` -> `core.commit` task -> Committer runner -> StateStore。
- `events` -> `core.event.append` task -> EventLog。
- `effects` -> `effect.*` task -> Effectful runner。
- `tasks` -> 显式进入 TaskPool。
- `values` / `resources` -> Resource/Event lineage facts。

复杂编排必须由插件 runner 显式返回后续 task 或监听 TaskEvent 后再提交 task；Core
不提供 TaskGroup、WaitSet、pipeline.run、broadcast.run、matcher.run 或 actor.send。

## 7. ResourceManager

控制面经过 Core，数据面不一定经过 Core。TaskPool 不搬运大数据本体。

资源和值统一以 descriptor 表达：

- `ValueRef`：小型结构化、可共享、可版本化值。
- `ResourceRef`：大型数据、blob、file-backed mmap、stream 或 provider-RPC 资源。
- `ResourceCellRef`：长期资源状态单元，例如连接池、stream、cookie jar、rate limiter。
- `ResourceLease`：某个 task step 临时使用 ResourceCell 的租约。
- `StateRef`：跨 task 权威语义状态。

默认规则：

- 共享资源 readonly / sealed。
- 修改产生新 ref 或 StateDelta。
- 原地写必须 ExclusiveWriteLease。
- Runner 可以持有短期 ResourceLease，但不能拥有 ResourceCell。

## 8. Plugin Loading

插件声明能力，RuntimeProfile 决定组合，resolver 生成确定性 load plan，Core 只校验和
物化。

Core 不负责插件扫描、下载、安装、依赖解算、版本选择、Python/npm/cargo 依赖管理或
运行组合策略。

Core 负责：

- 校验 `RuntimeLoadPlan`。
- 校验 runner descriptor 不超出 lock 授权。
- 构建并 freeze RunnerRegistry、HandlerBindingRegistry 和 contract surface。
- 记录 registry generation、plugin generation 和 contract fingerprint。

## 9. Hot Reload

热重载使用新 plugin generation，不原地替换对象。

Contract surface 兼容性：

- `Identical`：直接热重载。
- `Additive`：可热重载。
- `Deprecated`：可保留兼容处理，但禁止新增占用。
- `Removed`：必须 zero occupancy。
- `Breaking`：必须 migration、drain 或 restart。

切换 active generation 后，新 task 使用新 registry；旧 generation 不原地替换，也不在
存在污染/未知 running invocation 时提前 dispose。

## 10. Domain Neutrality

模拟个体不是 runtime 实例，而是上层 Store 中的数据聚合。Rust core 中不得出现
Yume、LLM、IM、MCP、ChatCompletion、OneBot 等领域或产品专用执行分支。
