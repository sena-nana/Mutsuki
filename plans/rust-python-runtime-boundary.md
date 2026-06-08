# Rust / Python 分层运行时边界

本文件记录 MutsukiBot 在 Tauri 桌面架构下的可演进分层：将通用 Agent
runtime mechanics 下沉到 Rust，同时保留 Python 插件生态、Yume 能力与外部协议
桥。本文是架构方向文档，不表示当前 Python Core 已完成迁移；任何字段级协议变更
仍必须同步更新 [contracts.md](contracts.md)。

## 1. 目标

Rust 层应成为可独立复用的 Agent runtime kernel。它服务两个上层形态：

```text
Mutsuki mode:
  Rust AgentRuntime
    -> Python PluginHost
    -> Mutsuki plugins / Yume plugins / transport plugins

Lilia mode:
  Rust AgentRuntime
    -> native Rust tools / direct app services
    -> no dynamic plugin system required
```

核心目标：

- 让 Tauri 应用直接拥有稳定的运行时、调度、状态与观测入口。
- 让 Lilia 式工程 Agent 可直接复用 Rust runtime，而不依赖 Mutsuki 的 Python
  插件系统。
- 让 Mutsuki / Yume 继续保留 Python AI 生态、动态插件、`RefPayload` by-ref
  传递与外部协议桥能力。
- 不把 Yume、Lilia、IM、LLM、MCP 或应用后端语义写进 Rust runtime core。

一句话边界：**Rust 管 runtime mechanics，Python 管 runtime semantics 与能力生态。**

## 2. 总体分层

```text
Tauri Frontend
  |
Rust App Host
  |
Rust AgentRuntime
  |-- Agent lifecycle state machine
  |-- Scheduler / inbox / cancellation / timeout
  |-- Routing / election / ScopeRule matching
  |-- Operation registry metadata
  |-- Capability and resource gate
  |-- Trace causal bookkeeping
  |-- Clock / IdGen / RNG
  |
RuntimeBackend boundary
  |
+----------------------------+----------------------------+
| Mutsuki Python Host        | Lilia Native Host          |
|----------------------------|----------------------------|
| PluginLoader               | Static tool registry       |
| PluginScope                | Workspace services         |
| Python Operation handlers  | Shell / git / editor tools |
| Python ExecutionStrategy   | Lilia planner / model loop |
| Python Handle ownership    | Lilia resource services    |
| Yume / LLM / transports    | Product-specific policy    |
+----------------------------+----------------------------+
```

Rust runtime 不应知道上层使用的是 Python 插件、Rust 工具、工程 Agent 还是
Yume 认知链路。它只通过 backend trait / protocol 推进策略与调用 Operation。

## 3. Rust 层职责

Rust 层负责可跨 Mutsuki 与 Lilia 复用的运行机制。

### 3.1 App Host

Tauri / Rust app host 负责：

- 启停 Python sidecar 或 native backend。
- 管理配置、日志目录、崩溃恢复与进程健康状态。
- 向前端暴露 dashboard / control plane command。
- 转发 UI 输入为内部 `Envelope` 或 backend command。
- 保持前端状态订阅，不让 UI 直接依赖 Python 插件实现。

App Host 不负责 Agent 行为、不解析外部协议语义、不持有插件资源事实。

### 3.2 AgentRuntime

AgentRuntime 负责：

- Agent 生命周期：`spawn / awake / sleep / stop` 状态机。
- Agent inbox、tick 调度、暂停、恢复、取消、超时与背压。
- 多 Agent 注册、生命周期过滤、owner 选择与候选排序。
- `ScopeRule` 对 `Envelope` 的纯数据匹配。
- Operation / Source registry 的元数据、状态与一致性检查。
- capability gate 与资源租约 gate。
- trace root、span parent-child 因果链、运行时事件输出。
- 决定性 clock / id / rng 注入。

AgentRuntime 不负责：

- 插件装载与热重载实现。
- Python 函数签名反射、`@operation` schema 生成。
- Yume thought loop、memory、affect、sleep、LLM、模型推理。
- Lilia 的文件编辑、shell、git、浏览器或模型策略语义。
- 外部协议翻译，如 OneBot、MCP、ChatCompletion。

### 3.3 Resource Gate

Rust 层可以管理资源治理事实：

- `ref_id`、`RefDescriptor`、owner、lease count。
- capacity / quota / acquire / release。
- keepalive / eviction 元数据。
- leak audit 与 dashboard 状态。

第一阶段 Rust 不直接持有 Python tensor、vLLM handle、socket 或 SDK client。
实际对象所有权仍在 backend 侧；Rust 只持有 token 与可序列化元数据。

## 4. Python 层职责

Python 层在 Mutsuki mode 下作为 `PythonPluginHost` 存在，负责动态能力生态。

Python 层负责：

- `PluginLoader`、DAG 拓扑、插件装载与卸载。
- `PluginScope` 与 Python 侧副作用回收。
- `@operation` / `@command` 装饰器、函数签名解析与 schema 生成。
- Operation handler、Source bridge、Service provider。
- Python `ExecutionStrategy` 实现，包括 Yume strategy。
- `Handle[T]` 的实际对象所有权与 finalizer。
- Yume latent / KV cache / PyTorch / vLLM / LLM provider。
- OneBot / MCP / ChatCompletion / browser / file system 等 reference 或领域插件。

Python 层不负责：

- 全局 Agent 生命周期状态机的唯一事实。
- 多 Agent owner 选择的最终裁决。
- runtime clock / id / rng 的全局决定性来源。
- 绕过 Rust runtime 直接向 UI 声称 Agent 状态。

## 5. Lilia Native Host 职责

Lilia 可不使用 Python 插件系统，直接把 Rust runtime 作为工程 Agent 底座。

Lilia 层负责：

- coding strategy / planner / model loop。
- workspace、shell、git、browser、editor、patch apply 等工具服务。
- 工程任务权限、审批、文件系统策略。
- Lilia 产品状态、用户协作策略与模型 provider 选择。

Rust runtime 在 Lilia mode 中只负责运行机制，不内置 `Codex`、`Claude`、
`coding`、`git`、`shell` 等产品或工具语义。

## 6. RuntimeBackend 接口

Rust runtime 与上层 backend 通过协议接口通信。接口命名为方向草案，具体字段以
后续 contracts 更新为准。

### 6.1 StrategyBackend

```text
StrategyBackend:
  on_awake(agent_id, ctx) -> Result[None, Error]
  on_input(agent_id, envelope, ctx) -> Result[StrategyResult, Error]
  next_step(agent_id, ctx) -> Result[StrategyResult, Error]
  on_stop(agent_id, ctx) -> Result[None, Error]
```

约束：

- strategy 不能修改 `agent_id`、owner、participation 或 accepts。
- strategy 调用工具必须走 `OperationBackend.invoke`，不能直接调用兄弟能力实现。
- Rust runtime 负责取消、超时与 trace parent span；backend 必须把错误结构化返回。

### 6.2 OperationBackend

```text
OperationBackend:
  list_operations(agent_id) -> tuple[OperationDescriptor, ...]
  invoke(agent_id, op_id, payload, invoke_ctx) -> Result[Envelope | object, Error]
  operation_status(agent_id, op_id) -> OperationStatus
```

Mutsuki mode 中，`OperationBackend` 转发到 Python PluginHost。Lilia mode 中，
它可以是 native Rust tool registry。

约束：

- Rust runtime 持有 Operation 元数据与状态，不持有 Python callable 裸引用。
- 调用 Python handler 时必须通过 `plugin_id / op_id / generation` 间接索引，
  避免热重载后调用旧函数。
- handler 抛错必须转成结构化 `Error`，并让 operation 进入 unhealthy 或
  backend 声明的失败状态。

### 6.3 PluginBackend（仅 Mutsuki mode）

```text
PluginBackend:
  load_plugin(plugin_spec, config) -> Result[PluginManifest, Error]
  unload_plugin(plugin_id) -> Result[None, Error]
  list_plugins() -> tuple[PluginManifest, ...]
  list_sources() -> tuple[SourceDescriptor, ...]
  list_operations() -> tuple[OperationDescriptor, ...]
```

Rust runtime 不依赖该接口即可运行 Lilia mode；它只在 Mutsuki mode 下用于动态
插件生态。

### 6.4 ResourceBackend

```text
ResourceBackend:
  register(descriptor, owner) -> Result[RefId, Error]
  acquire(ref_id, requester, lease_policy) -> Result[LeaseToken, Error]
  release(ref_id, lease_token) -> Result[None, Error]
  list_records(filter) -> tuple[ResourceRecord, ...]
```

第一阶段推荐事实分工：

- Python `Handle` 是实际对象所有权事实。
- Rust Resource Gate 是资源治理事实。
- Python borrow 前先向 Rust acquire，borrow 退出后在 `finally` 中 release。
- release 失败不能被吞掉，必须记录结构化错误。

## 7. Rust / Python 互操作规则

### 7.1 只跨边界传纯协议

Rust 与 Python 之间只传：

- `Envelope`
- `OperationDescriptor`
- `SourceDescriptor`
- `TraceSpan`
- `Error`
- `StrategyResult`
- `RefDescriptor`
- `ref_id` / `lease_token`
- 可序列化 payload

禁止跨边界传：

- Python tensor / latent 实体。
- vLLM KV cache 真实 handle。
- socket、SDK client、数据库连接。
- Python callable 裸引用。
- `RefPayload` 中的实际 `Handle[T]`。

### 7.2 RefPayload 不跨进程

`RefPayload` 的真实对象引用只在同进程、同信任域内传递。若 Rust host 与 Python
core 是跨进程部署，则边界上只能传 `ref_id`、`RefDescriptor` 与租约 token。

```text
跨进程:
  ref_id + descriptor + lease_token

Python 进程内:
  RefPayload + Handle[T] + borrow()
```

若某条路径要求把 latent / KV cache 序列化才能跨 Rust/Python 边界，应视为架构
错误，优先调整边界或契约。

### 7.3 Finalizer 归属

第一阶段 finalizer 归属 backend。Python 创建的资源由 Python finalizer 释放；
Rust 创建的资源由 Rust finalizer 释放。

Rust 可以发出 eviction / release 请求，但不应在第一阶段直接回调 Python
finalizer。这样可以避免 GIL、asyncio、插件卸载 generation 与异常桥接交织。

### 7.4 热重载 generation

Rust runtime 不保存 Python callable。所有 Python handler 调用必须使用稳定
间接键：

```text
plugin_id
plugin_generation
op_id
handler_id
```

Python PluginHost 在卸载插件时使旧 generation 失效。Rust runtime 调用旧
generation 时必须 fail-loud 为结构化错误，而不是尝试 fallback 到新 handler。

### 7.5 Async 与取消

Rust runtime 是取消和超时的源头。跨 Python 调用时必须携带：

- deadline / timeout。
- trace context。
- cancellation token。
- invoke id。

Python backend 必须在可等待边界检查取消；如果底层库不可取消，应返回
`Error` 并在 trace 中记录不可取消证据。

## 8. Agent 流位置

在 Rust 化到运行逻辑层后，Agent 主流位于 Rust：

```text
Envelope enters Rust AgentRuntime
  -> route / select owner
  -> enqueue Agent inbox
  -> scheduler ticks Agent
  -> calls StrategyBackend.on_input or next_step
  -> backend may call OperationBackend.invoke
  -> StrategyResult returns
  -> Rust updates Agent state and trace
```

但 Agent 行为语义不在 Rust：

- Mutsuki mode：行为由 Python `ExecutionStrategy` 与插件决定。
- Lilia mode：行为由 Lilia native strategy / planner 决定。

因此 Rust 运行的是“主体与调度”，不是“Yume 怎么思考”或“Lilia 怎么写代码”。

## 9. 与现有 Mutsuki 契约的关系

本分层必须保持以下现有 hard rules：

- Agent 仍是一等运行时实体。
- LLM、记忆、情感、睡眠、IM、MCP、ChatCompletion 不进入 runtime core。
- Operation 仍是工具、命令与跨能力调用的统一入口。
- 插件之间仍只能通过契约、服务、Operation 通信。
- 未声明 capability / accepts 的调用或路由仍需 fail-loud 或显式拒绝。
- 决定性时间与 ID 仍由 runtime 注入。
- `RefPayload` / `Handle` 不被强制序列化，不跨进程传真实对象。
- trace 因果链必须跨 Rust / Python backend 保持闭合。

## 10. 迁移路线

### Phase R1: Rust App Host

- Tauri host 管理 Python sidecar。
- UI 通过 Rust host 查询状态与日志。
- Python Core 仍是唯一 Agent runtime。

### Phase R2: Rust Trace Service

- Python 发出 `TraceSpan`。
- Rust 负责 trace 写入、读取、索引与 dashboard stream。
- 主链路不依赖 trace sink 成功。

### Phase R3: Rust Resource Gate

- Rust 管 `RefDescriptor`、租约、quota、leak audit。
- Python 仍持有实际 `Handle[T]` 与 finalizer。
- `RefPayload` 真实对象不跨进程。

### Phase R4: Rust AgentRuntime Kernel

- Agent lifecycle、scheduler、routing、election、capability gate 下沉 Rust。
- Python PluginHost 作为 StrategyBackend / OperationBackend。
- Lilia 可直接接 native backend 使用同一 runtime。

### Phase R5: Selective Native Fast Paths

- 只把已证明确认为瓶颈的路径下沉 Rust。
- 候选包括 ScopeRule matching、operation registry、trace bookkeeping、资源策略。
- 不因性能把领域语义写入 runtime core。

## 11. 验收标准

任一 Rust 分层实现必须满足：

- 不装载 Python PluginHost 时，Rust runtime 可用 Lilia native backend 跑通最小
  Agent step loop。
- 装载 Python PluginHost 时，现有 Mutsuki plugin / Operation / Source 语义保持。
- Yume 样本中的 latent / KV cache 通过 token + Python 内 by-ref 传递，不被序列化。
- Rust / Python trace span 父子链闭合，可由 contract test kit 回放检查。
- Python plugin reload 后，Rust 不调用旧 generation handler。
- 资源 acquire / release 双边一致；release 失败必须结构化记录。
- Rust core 搜不到 Yume、latent、tensor、gpu、Lilia、Codex、OneBot、MCP 等领域
  或产品专用执行分支。

## 12. 反向判定

出现以下情况应停止迁移并修正边界：

- Rust runtime 需要理解 Yume latent / KV cache 字段才能调度。
- 为了跨 Rust/Python 调用而要求序列化真实 `Handle[T]`。
- Python 插件热重载后 Rust 仍持有旧 callable。
- Lilia 工具语义进入 Rust runtime core。
- Rust scheduler 绕过 backend 协议直接调用插件实现。
- trace 在 Rust / Python 边界断链且无法由结构化错误解释。
- 为性能绕过 capability / permission / scope / trace 拦截链。

本设计的目标是让 Rust 成为可复用的运行时骨架，而不是把 Mutsuki 插件生态或
Lilia 工程语义提前固化进 core。
