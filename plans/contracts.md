# NanoBot 内部协议草案（v0.0）

本文件回答：**core 与 plugins 之间用什么协议通信、协议长什么样、哪些字段是必须的**。

> v0.0 阶段不锁字段名与具体实现，**只列结构骨架与必须预留的位**。任何字段在 v0.1 引入实现时可调整。

## 0. 协议总原则

- 所有协议对象使用 `msgspec.Struct`。
- 必含字段：版本号 (`schema_version`)、对象 ID、时间戳。
- 序列化、版本号、错误结构必须从 v0.0 起预留位。
- 偏好显式枚举、强类型 payload、版本 ID、结构化错误，胜过下游需要解析的自由文本字符串。
- 协议变化必须同 PR 内更新本文件。

## 1. 核心契约对象（v0.0 必须草拟）

| 对象 | 用途 | 关键字段（草案） |
|---|---|---|
| `AgentId` | Agent 身份 | `value: str`（kebab-case，含命名空间） |
| `AgentContext` | Agent 运行期上下文 | `agent_id`、`clock`、`id_gen`、`rng`、`services`、`scope`、`trace_ctx` |
| `LifecyclePhase` | 生命周期阶段 | enum：`spawn / awake / sleep / stop` |
| `Message` | 入站 / 出站消息 | `id`、`timestamp`、`source`、`channel`、`content_parts`、`capabilities_required` |
| `Event` | 内部事件 | `id`、`timestamp`、`type`、`source_plugin`、`payload`、`trace_id`、`span_id`、`parent_span_id` |
| `Capability` | 能力声明 | `name: str`、`quantity: optional`、`policy: optional` |
| `Service` | 服务声明 | `name`、`contract_id`、`mode: by_value \| by_ref`、`version` |
| `PluginManifest` | 插件清单 | 详见 §2 |
| `Decision` | Agent / 插件做出的决策 | `id`、`source`、`route`、`payload`、`alternatives_considered` |
| `Error` | 结构化错误 | 详见 §3 |
| `TraceSpan` | trace 因果链节点 | `trace_id`、`span_id`、`parent_span_id`、`name`、`start`、`end`、`attributes`、`status` |
| `RefPayload[T]` | 标记字段为「按引用传递」（详见 §11） | `ref_id`、`handle: Handle[T]`、`descriptor: RefDescriptor` |
| `Handle[T]` | 引用所有权与生命周期 | `acquire / release / borrow / is_alive`；与 `PluginScope` 集成 |
| `RefDescriptor` | 引用的可观测元数据 | `ref_id`、`kind`、`schema_id`、`schema_version`、`attributes`、`lineage` |
| `BackpressureChannel[T]` | 流式 payload 速率匹配 | `send / recv / high_watermark / low_watermark` |

## 2. PluginManifest

```text
PluginManifest:
  id: str                           # kebab-case，全局唯一
  version: SemVer
  contracts: list[ContractDep]      # 依赖的契约包及版本范围
  capabilities: list[Capability]    # 申报的能力（含资源量纲）
  provides_services: list[Service]
  requires_services: list[ServiceDep]
  requires_plugins: list[PluginDep] # 用于 DAG 拓扑加载
  config_schema: type[msgspec.Struct]
  entrypoints:
    on_load: callable
    on_unload: callable
    commands: list[CommandSpec]     # 由「指令即工具」装饰器自动收集
```

约束：

- `id` 全局唯一；`version` 遵循 SemVer。
- `contracts / capabilities / config_schema` 缺一不可；core 在装载前校验，不通过则拒绝装载。
- `requires_plugins` 用于 DAG 解析；存在环则拒绝启动。
- `commands` 由 `@plugin.command` / `@plugin.tool` 装饰器自动填充（详见 [engineering.md §3.3](engineering.md#33-指令即工具hard-rule)）。

## 3. Error（结构化错误）

```text
Error:
  code: str                          # 稳定字符串，可用于路由 / 测试断言
  source: str                        # 产生错误的插件 / 服务 / 模块
  route: str                         # 触发路径（如 "kernel.text → runtime.inference"）
  lost_capability: optional[str]     # 因错误丢失的能力（如 "latent" → fallback "text"）
  recovery: optional[RecoveryAction] # core / 上游可执行的恢复动作
  cause: optional[Error]             # 因果链
  evidence: dict                     # 诊断证据（堆栈摘要、关键变量、trace_id）
```

约束：

- 错误是一等数据对象，不是字符串。
- fallback 必须显式记录原因。
- **不允许吞异常返合理默认值**。

### 3.1 标准错误码（v0.0 草案）

核心保留以下错误码命名空间，插件可在自有命名空间扩展：

| code | 触发场景 |
|---|---|
| `capability.not_declared` | 未在 manifest 申报即调用对应能力 |
| `capability.exhausted` | 资源量纲超限 |
| `schema.mismatch` | 契约 `schema_id` / `schema_version` 不兼容（兼容规则由契约包注册回调） |
| `handle.leak` | `PluginScope` / `TransactionScope` 关闭时仍有未释放 `Handle` |
| `handle.use_after_release` | 使用已释放的 `Handle` |
| `ref.cross_domain` | 携带 `RefPayload` 字段的契约试图跨进程 / 跨隔离域传递 |
| `ref.serialize_attempt` | 试图序列化 `RefPayload` 字段 |
| `plugin.cycle` | 插件 DAG 存在环 |
| `plugin.scope_violation` | 副作用未通过 `PluginScope` 注册 |
| `transaction.compensation_failed` | Saga 补偿步骤失败 |

## 4. Capability 草案能力名

v0.0 草案，可在 v0.1 扩展：

| 名称 | 量纲示例 | 用途 |
|---|---|---|
| `read_message` | — | 读消息 |
| `send_message` | — | 发消息 |
| `call_llm` | `{"tokens_per_min": 60000}` | 调用 LLM |
| `persist` | `{"bytes": "100MiB"}` | 写持久化 |
| `network_egress` | `{"hosts": ["api.openai.com"]}` | 出站网络 |
| `spawn_agent` | `{"max": 4}` | 创建 Agent |
| `vram` | `{"bytes": "8GiB"}` | GPU 显存 |
| `kv_slot` | `{"slots": 4}` | KV cache 槽位 |
| `train_quota` | `{"jobs": 1}` | 训练任务配额 |
| `hold_ref` | `{"max_handles": 64}` | 长期持有 `Handle`（跨 tick） |
| `borrow_ref` | — | 短期借用 `Handle`（单次调用内） |
| `produce_ref_stream` | `{"channels": 4}` | 作为 `BackpressureChannel` 生产者 |

约束：

- 未在 manifest 申报即调用对应能力 → core 拒绝并产生 `Error(code="capability.not_declared")`。
- 量纲化能力由 `ResourceGovernor` 协商；超额则排队或拒绝。

## 5. 服务注入语义

```text
ServiceMode:
  by_value   # 可序列化、可跨进程；走 codec
  by_ref     # 同进程对象引用、零拷贝；不可跨进程
```

- `by-ref` 用于 GPU 句柄、KV cache、模型实例等。
- `by-value` 用于配置、纯数据、跨进程消息。
- 服务契约必须显式声明 mode；混用需通过适配器服务。

## 6. 事务原语

```text
TransactionScope:
  enter() -> handle
  register_compensation(callable)
  commit()
  rollback()

Saga:
  add_step(forward, compensate)
  run() -> Result
  on_failure -> 自动按顺序 compensate
```

适用场景：

- `sleep → collect → evaluate → compile → integrate → rollback`
- adapter / patch 版本切换 → Agent atomic_swap
- 任何「失败时必须撤销前置副作用」的多步骤流程

## 7. PluginScope（资源生命周期追踪）

```text
PluginScope:
  add_subscription(handle)
  add_timer(handle)
  add_service_registration(handle)
  add_context_attachment(handle)
  add_config_watcher(handle)
  close()  # 自动释放全部
```

约束：

- 所有插件副作用必须经 scope 注册。
- 卸载时 core 调用 `scope.close()`，未释放即 panic。
- Lint 规则（v0.1）将检查直接调用全局 `bus.subscribe(...)`、`loop.create_task(...)` 不带 scope 参数等模式。

## 8. Trace 因果链

```text
TraceSpan:
  trace_id: str       # 一次外部触发的全局 ID
  span_id: str        # 本次调用的 ID
  parent_span_id: str # 父调用 ID（跨插件因果链）
  name: str           # "plugin.echo.handle" 等
  start: timestamp
  end: optional[timestamp]
  attributes: dict
  status: ok | error
```

约束：

- 每个命令调用、工具调用、Agent tick、生命周期切换都产生 span。
- 跨插件传递时 `parent_span_id` 必须连接，断链视为 bug。
- observability 插件订阅消费。

## 9. 决定性时间与 ID

`AgentContext` 必须暴露：

```text
context.clock       # Clock 接口：now() / monotonic() / sleep()
context.id_gen      # IdGen 接口：next() -> str（决定性 / 可种子化）
context.rng         # RNG 接口：可种子化
```

约束：

- 插件禁止直接使用 `time.time()` / `uuid.uuid4()` / `random` 全局源。
- 测试时替换为可控时钟与确定性 ID 源即可获得可重放性。

## 10. Schema 协议

> **设计原则**：Schema 兼容性是**所有契约对象**的通用属性，不限于 [§11](#11-通用-by-ref-payload-协议) 的 ref-by 协议。任何 `msgspec.Struct` 契约都可声明 `schema_id` + `schema_version`，框架据此做版本协商与不兼容拦截。

### 10.1 字段约定

任一契约对象可在 metadata 中声明：

```text
schema_id: str            # 完整契约标识（如 "yume.thought.packet" / "nanobot.message"）
schema_version: str       # SemVer
```

约束：

- `schema_id` 全局唯一；建议用 `<namespace>.<concept>` 命名。
- `schema_version` 遵循 SemVer。
- 同一 `schema_id` 的不同 `schema_version` 之间是否兼容由契约包注册回调决定（见 §10.2）；核心**不内置**任何兼容规则。

### 10.2 兼容回调注册

```text
register_schema_compatibility(
    schema_id: str,
    is_compatible: callable[[producer_version: str, consumer_version: str], bool],
)
```

约束：

- 契约包在加载时注册回调；未注册则默认仅 byte-equal 版本兼容。
- 回调由契约包负责，例如 `nanobot-contracts-yume` 自行决定「latent space v1.2 兼容 v1.3」。
- 核心代码搜不到任何具体 `schema_id` 的兼容判断逻辑。

### 10.3 不兼容拦截

- 发布与订阅的 `schema_id` 不同 → `Error(code="schema.mismatch")`。
- `schema_id` 相同但 `schema_version` 不同 → 调用 §10.2 注册的回调；返回 false 同样产生 `Error(code="schema.mismatch")`。
- 装载时（DAG 解析阶段）静态校验已知发布 / 订阅对；运行时 codec 再校验一次。

### 10.4 与 [§11](#11-通用-by-ref-payload-协议) 的关系

`RefDescriptor` 通过 §10 的机制声明其引用对象的 `schema_id` / `schema_version`，从而复用同一套兼容判断逻辑。除此之外 §10 是独立通用机制，不依赖 ref 语义。

## 11. 通用 By-Ref Payload 协议

> **设计原则**：核心层只提供通用的「按引用传递」机制，**不假设引用对象的语义**。
> Yume 的 latent / KV cache、mind-sim 的会话句柄、未来任何插件的非序列化对象都
> 是该机制的应用。具体语义由领域契约包（如 `nanobot-contracts-yume`）定义。

### 11.1 RefPayload[T] —— 字段标记

```text
RefPayload[T]:
  ref_id: str                 # 全局唯一 ID（runtime 注入）
  handle: Handle[T]           # 实际对象的所有权句柄
  descriptor: RefDescriptor   # 可观测元数据
```

约束：

- 在契约 `msgspec.Struct` 中以特殊标记声明：例如 `field: Annotated[RefPayload[Any], RefField()]`。
- **不参与序列化**：codec 遇到 `RefPayload` 字段时，要么抛 `Error(code="ref.serialize_attempt")`，要么仅写出 `descriptor` 部分（取决于 codec 模式）。
- **不可跨进程 / 跨隔离域**：装载时静态校验，运行时再校验一次。
- T 是任何 Python 对象，核心不约束。

### 11.2 Handle[T] —— 所有权与生命周期

```text
Handle[T]:
  acquire() -> T              # 获取强引用；引用计数 +1
  release()                   # 释放引用；引用计数 -1，归零时调用 finalizer
  borrow() -> ContextManager[T]  # 短期借用，with 块退出自动释放
  is_alive() -> bool
  attach_to(scope: PluginScope | TransactionScope)  # 绑定到 scope，scope 关闭时自动释放
```

约束：

- 任何 `Handle` 必须 `attach_to` 一个 scope；未绑定则在创建处立即报 `Error(code="handle.leak")`。
- `release` 后再使用 → `Error(code="handle.use_after_release")`。
- 与 `PluginScope` / `TransactionScope` 集成，scope 关闭时自动调用 `release` 清理。
- finalizer 由创建者注册（如 `lambda obj: obj.free_gpu_memory()`），核心不假设清理逻辑。

### 11.3 RefDescriptor —— 可观测元数据

```text
RefDescriptor:
  ref_id: str
  kind: str                          # 领域命名（如 "yume.latent" / "mindsim.session"）
  schema_id: str                     # 完整契约标识（如 "yume.latent/v2"）
  schema_version: str                # SemVer
  attributes: dict[str, JSON]        # 领域元数据（如 {"shape": [4096], "dtype": "fp16", "device": "cuda:0"}）
  lineage: list[str]                 # 派生自哪些 ref_id（用于因果追溯）
```

约束：

- 框架使用 `RefDescriptor` 做 trace、审计、面板显示，**不解读 `attributes` 内容**。
- `kind` 用于面板分类与权限策略匹配，不参与执行逻辑。
- `lineage` 在 `Handle` 派生（如从一个 latent 算出另一个 latent）时由插件填充。

### 11.4 Schema 兼容性

`RefDescriptor` 的 `schema_id` / `schema_version` 复用 [§10 Schema 协议](#10-schema-协议) 的通用机制（统一拦截、兼容回调注册、核心不内置兼容规则）。本节不重复定义。

### 11.5 Replayability 声明

任何契约都可在元数据中声明：

```text
Replayability: full | input_seed_only | none
```

- `full` —— 给定输入与种子可完整重放（默认）。
- `input_seed_only` —— 仅可由原始输入 + 种子重新驱动，不保证 byte-equal。
- `none` —— 不可重放（典型：含模型非确定性的 latent 流）。

约束：

- replayer 严格尊重声明，**从不假装能回放 `none`**。
- 不可重放并非 bug；测试与审计基于 `RefDescriptor.attributes` 的可观测性，而非 byte-equal。

### 11.6 BackpressureChannel[T]

```text
BackpressureChannel[T]:
  send(item: T) -> Awaitable      # 高水位时阻塞生产者
  recv() -> Awaitable[T]
  high_watermark: int
  low_watermark: int
  closed: bool
```

约束：

- 生产者必须申报 `produce_ref_stream` capability。
- channel 自身可绑定到 `PluginScope`，scope 关闭即关闭 channel。
- 适用于任何流式 payload，不限 ref。

### 11.7 跨进程 / 跨域校验

- 装载时（DAG 解析阶段）：核心扫描所有契约字段，若一个契约含 `RefPayload` 字段且其发布者与订阅者属于不同进程 / 不同隔离域，**直接拒绝装载**，错误指向违规插件 manifest。
- 运行时：codec 序列化任何含 `RefPayload` 字段的对象 → `Error(code="ref.serialize_attempt")`。
- 跨进程通信只能传 `RefDescriptor`（已序列化元数据），由对端按需重建。

### 11.8 Trace 自动降级

- TraceSpan 写入处自动用 `RefDescriptor` 替换 `RefPayload`。
- 插件无需关心；不可通过配置关闭（防止意外把 raw 引用持久化到日志）。

### 11.9 泄漏检测

- `PluginScope.close()` / `TransactionScope.commit()` / `TransactionScope.rollback()` 时枚举仍未释放的 `Handle`。
- 存在未释放即产生 `Error(code="handle.leak")`，含违规 ref_id 列表与 lineage。
- observability 插件可订阅 leak 事件做周期性巡检。

### 11.10 与 Service.mode 的互补关系

| 场景 | 选择 |
|---|---|
| 长生命周期共享对象（推理引擎实例、KV 管理器） | `Service.mode = by_ref` |
| 调用间瞬态对象（一次推理产生的 latent、一次会话产生的 KV slot） | `RefPayload[T]` + `Handle[T]` |
| 跨进程值（配置、纯数据消息） | `Service.mode = by_value` |

二者互补，不互斥：`by_ref` 的 service 可以返回 `RefPayload`。

## 12. 承载性测试样本（contracts 验证用）

设计任何契约时，必须用以下样本验证可承载：

- **Yume**：`StimulusEvent`、`ThoughtPacket`、`KernelRequest`、`KernelResult`、`ExpressionDecision`、`SleepCandidate`、`SleepArtifact`
- **mind-sim**：`bus` 消息、`engine` 请求、`provider` 调用、`server` 会话

参考阅读（**禁止复制代码**）：

- [Yume/yume/contracts/](../../../Yume/yume/contracts/) —— `thought.py` / `state.py` / `kernel.py` / `events.py` / `awake.py` / `sleep.py`
- [Yume/mind-sim/mind_sim/](../../../Yume/mind-sim/mind_sim/) —— `bus` / `engines` / `providers` / `server`

契约若无法承载这些类型 → **修契约**，不为任一系统特化。
