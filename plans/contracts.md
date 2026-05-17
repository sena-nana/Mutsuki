# MutsukiBot 内部协议草案（v0.0）

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
| `Message` | 入站 / 出站消息 | `id`、`timestamp`、`source`、`parts`、`capabilities_required`（v0.1 实现：[mutsukibot/contracts/message.py](../mutsukibot/contracts/message.py)；字段名 `parts` 为正式名，本表早期草案的 `content_parts` 已弃用） |
| `Event` | 内部事件 | `id`、`timestamp`、`type`、`source_plugin`、`payload`、`trace_id`、`span_id`、`parent_span_id` |
| `Capability` | 能力声明 | `name: str`、`quantity: optional`、`policy: optional` |
| `Service` | 服务声明 | `name`、`contract_id`、`mode: by_value \| by_ref`、`version` |
| `PluginManifest` | 插件清单 | 详见 §2 |
| `Decision` | Agent / 插件做出的决策 | `id`、`source`、`route`、`payload`、`alternatives_considered` |
| `Error` | 结构化错误 | 详见 §3 |
| `TraceSpan` | trace 因果链节点 | `trace_id`、`span_id`、`parent_span_id`、`name`、`start`、`end`、`attributes`、`status` |
| `RefPayload[T]` | 标记字段为「按引用传递」（详见 §11） | `ref_id`、`handle: Handle[T]`、`descriptor: RefDescriptor` |
| `Handle[T]` | 引用所有权与生命周期 | `acquire / release / borrow / is_alive`；与 `PluginScope` 集成 |
| `RefDescriptor` | 引用的可观测元数据 | `ref_id`、`kind`、`schema_id_target`、`schema_version_target`、`attributes`、`lineage`（详见 §11.3） |
| `BackpressureChannel[T]` | 流式 payload 速率匹配 | `send / recv / high_watermark / low_watermark` |
| `Envelope` | 通用入站/出站载体（IM `Message` 是其子类） | `id`、`timestamp`、`source: SourceRef`、`payload_schema_id`、`capabilities_required`（详见 §16） |
| `SourceRef` | 通用事件来源描述（IM `ChannelRef` 是其子类） | `source_id`、`kind: SourceKindName`、`metadata`（详见 §16） |
| `OperationDescriptor` | 静态声明的可调用 Operation | `op_id`、`perms_rule_id`、`requires_capabilities`、`params_schema`、`return_schema`、`is_tool`（详见 §14；命令 `CommandSpec` 是其特化）|
| `OperationDep` | 插件依赖外部 Operation 的声明 | `op_id`、`required_caps`（用于 DAG 反向解析）|
| `SourceKindName` | 已注册的 Source 类型名（`im` / `tool` / `hybrid`） | `RegisteredString` 子类，门面 `SourceKinds` |
| `SourceDescriptor` | 静态声明的事件源 | `source_id`、`kind`、`capabilities`（详见 §15）|
| `SourceDep` | 插件依赖外部 Source 的声明 | `source_id`、`required_caps` |
| `ScopeRule` | envelope 路由谓词（与 `PermissionRule` 同模式） | `_Leaf / _And / _Or` AST + `check(envelope) -> bool`（详见 §17） |
| `ScopeName` | 已注册的命名 scope（门面 `Scopes`） | `RegisteredString` 子类 |

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
- `commands` 由 `@command` 装饰器自动填充（详见 [engineering.md §3.3](engineering.md#33-指令即工具hard-rule)）。

**v0.1 实现层规范**：manifest 字段以 `ClassVar` 形式声明在 `Plugin` 子类
（`id` / `version` / `capabilities` / `requires_plugins` / `requires_services` /
`provides_services` / `contracts`），`Config` 是嵌套的 `msgspec.Struct` 类。
`PluginMeta` 元类在 class 定义时校验上述字段并自动构造 `PluginManifest`，绑定到
`cls.__manifest__`；用户不直接构造 `PluginManifest` 实例。

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
| `ref.not_found` | `RefArg` / `ResourceHost` 无法找到指定引用 |
| `ref.kind_mismatch` | `RefArg` / `ResourceHost` 找到的引用 `kind` 与声明不匹配 |
| `ref.cross_domain` | 携带 `RefPayload` 字段的契约试图跨进程 / 跨隔离域传递 |
| `ref.serialize_attempt` | 试图序列化 `RefPayload` 字段 |
| `plugin.cycle` | 插件 DAG 存在环 |
| `plugin.scope_violation` | 副作用未通过 `PluginScope` 注册 |
| `transaction.compensation_failed` | Saga 补偿步骤失败 |
| `operation.not_found` | dispatcher.invoke 找不到对应 op_id |
| `operation.undeclared` | 运行时注册的 op_id 未在调用方 plugin 的 `provides_operations` 静态声明集内 |
| `operation.conflict` | 多个 plugin 静态声明同一 op_id |
| `operation.unhealthy` | Operation 处于 unhealthy 状态（handler 此前抛出异常）|
| `operation.invoke_failed` | dispatcher.invoke 执行链路失败（capability/permission 拦截以外的失败）|
| `operation.handler_raised` | Operation handler 抛出未捕获异常 |
| `agent.not_found` | `dispatch.invoke_in_agent` 找不到目标 Agent |
| `source.unregistered` | publish envelope 时 `envelope.source.source_id` 不在已注册集 |
| `source.conflict` | 多个 plugin 静态声明同一 source_id |
| `source.undeclared` | 运行时注册的 source_id 未在调用方 plugin 的 `provides_sources` 静态声明集内 |
| `scope.no_match` | 路由阶段 envelope 无任何 Agent.accepts 匹配（仅在 strict 模式记录；默认 silently drop） |
| `trace.record_invalid` | JSONL trace 记录无法转换为 `TraceSpan` |
| `trace.replay_failed` | trace replay kit 发现重复 span、父链缺失、时间区间非法等因果错误 |

## 4. Capability 命名

**v0.1 实现**：`CapabilityName` 是 `str` 子类 + 进程内注册表（详见
[mutsukibot/contracts/capability.py](../mutsukibot/contracts/capability.py)）。
框架内置常量集中在 `Caps` 门面（[capability_builtin.py](../mutsukibot/contracts/capability_builtin.py)），
插件通过 `CapabilityName.register(name, declared_by=...)` 扩展自有命名空间
（如 `yume.vram` / `mindsim.session`）。**禁止用裸字符串构造 Capability**；
未注册的名字在 `CapabilityName(value)` 构造点立即抛 `UnknownCapabilityError`，
跨 owner 重注册抛 `CapabilityConflictError`。

v0.0 草案能力名（v0.1 已注册到 `Caps` 内）：

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
| `im.text` | — | IM 文本消息 Source/Operation |
| `im.image` | — | IM 图片消息 Source/Operation |
| `im.audio` | — | IM 音频消息 Source/Operation |
| `im.file` | — | IM 文件消息 Source/Operation |
| `im.markdown` | — | IM Markdown 富文本 |
| `im.card` | — | IM 卡片消息 |
| `im.reaction` | — | IM 表情/反应 |
| `im.typing` | — | IM 输入指示 |
| `tool.invoke` | — | MCP 风格工具型 Operation |
| `tool.event` | — | MCP 风格工具型 Source（外部状态变更推送）|

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
schema_id: str            # 完整契约标识（如 "yume.thought.packet" / "mutsukibot.message"）
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
- 回调由契约包负责，例如 `mutsukibot-contracts-yume` 自行决定「latent space v1.2 兼容 v1.3」。
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
> 是该机制的应用。具体语义由领域契约包（如 `mutsukibot-contracts-yume`）定义。

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
  ref_id: RefId
  kind: str                                # 领域命名（如 "yume.latent" / "mindsim.session"）
  schema_id_target: str                    # 引用对象的契约标识（如 "yume.latent/v2"）
  schema_version_target: str               # SemVer
  attributes: dict[str, str|int|float|bool] # 领域元数据（如 {"shape": "[4096]", "dtype": "fp16"}）
  lineage: tuple[RefId, ...]               # 派生自哪些 ref_id（用于因果追溯，不可变）
```

约束：

- 框架使用 `RefDescriptor` 做 trace、审计、面板显示，**不解读 `attributes` 内容**。
- `kind` 用于面板分类与权限策略匹配，不参与执行逻辑。
- `lineage` 在 `Handle` 派生（如从一个 latent 算出另一个 latent）时由插件填充。
- **字段命名**：`schema_id_target` / `schema_version_target` 而不是裸 `schema_id` / `schema_version`，
  避免与 [§10.1](#101-字段约定) 通用 schema 元数据字段（任何 Contract 自身都有）的同名歧义；
  `_target` 后缀表示「引用所指向对象的 schema」。
- `lineage` 用 `tuple` 而非 `list`，使 `RefDescriptor` 可哈希、可作为 trace key。

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

### 11.11 RefArg 类型化注入（v0.3 后续）

命令 / Operation 签名可声明：

```python
resource: Annotated[Handle[T], RefArg(kind="domain.resource")]
```

规则：

- `RefArg` 只认领 `Annotated[Handle[T], RefArg(...)]`；非 `Handle` 注解不得被当作 ref 参数。
- `source=payload` 时，调用 payload 中同名字段必须是 `Handle[T]` 或 `RefPayload[T]`，并且 `handle.descriptor.kind == RefArg.kind`。
- `source=resource_host` 时，通过 `ctx.services.resolve(ResourceHost, name=host_name)` 找到 host，再按 `ref_id`（或同名 payload 字段）解析句柄。
- 找不到引用 → `Error(code="ref.not_found")`；kind 不匹配 → `Error(code="ref.kind_mismatch")`。
- 解析 ResourceHost 句柄必须写 `resource_host.get_handle` trace span，并继承当前 `ctx.trace_ctx`。

## 12. Permission 系统（v0.1 引入）

> **设计原则**：与 Capability **正交**。Capability 是「插件**有能力**做某事」的
> *静态* manifest 申报；Permission 是「**当下这个调用**是否被允许」的 *动态* 谓词。

### 12.1 PermissionRule —— 谓词组合

```text
PermissionRule:
  __and__ -> 合并所有 checker（AND）
  __or__  -> 包装为新 OR 节点
  check(ctx) -> bool
```

实现见 [mutsukibot/contracts/permission.py](../mutsukibot/contracts/permission.py)。

### 12.2 PermissionName —— 命名权限注册式

与 `CapabilityName` 同模式：`str` 子类 + 注册表 + 内置门面 `Perms`
（`Perms.PUBLIC` / `Perms.AGENT_OWNER`）。注册时绑定 checker 函数。
插件可调 `PermissionName.register(name, declared_by=..., checker=...)` 扩展。

### 12.3 消费点

任意 `@command` / `@tool` / Service 方法都可声明：

```python
@command(perms=Perms.AGENT_OWNER & MyPerms.IN_CHANNEL("ops"))
async def restart(self, ctx: AgentContext) -> str: ...
```

scheduler 在路由前 `await rule.check(ctx)`；失败 → `Error(code="permission.denied")`。

## 13. 承载性测试样本（contracts 验证用）

设计任何契约时，必须用以下样本验证可承载：

- **Yume**：`StimulusEvent`、`ThoughtPacket`、`KernelRequest`、`KernelResult`、`ExpressionDecision`、`SleepCandidate`、`SleepArtifact`
- **mind-sim**：`bus` 消息、`engine` 请求、`provider` 调用、`server` 会话

参考阅读（**禁止复制代码**）：

- [Yume/yume/contracts/](../../../Yume/yume/contracts/) —— `thought.py` / `state.py` / `kernel.py` / `events.py` / `awake.py` / `sleep.py`
- [Yume/mind-sim/mind_sim/](../../../Yume/mind-sim/mind_sim/) —— `bus` / `engines` / `providers` / `server`

契约若无法承载这些类型 → **修契约**，不为任一系统特化。

## 14. Operation 协议（v0.2 引入）

> **设计原则**：命令（v0.1 `@command`）与跨 plugin RPC（v0.2 `dispatch.invoke`）是同一概念 —— 一个有「身份 + 准入 + 资源量纲 + 参数 schema + 实现」的可调用物。统一称为 **Operation**，对应 [AGENTS.md hard rule #5](../AGENTS.md)「指令即工具，禁止维护两份」。

### 14.1 OperationDescriptor —— 静态声明

```text
OperationDescriptor:
  op_id: str                        # agent-local 全限定，约定 "<plugin_id>.<name>" 或 "<source_namespace>.<name>"
  description: str                  # 人类可读，docstring 首段 / Annotated[T, Arg(desc=...)]
  perms_rule_id: str                # 关联到 PermissionRule
  requires_capabilities: tuple[CapabilityName, ...]
  params_schema: dict[str, Any]     # JSON Schema 形式（与 v0.1 CommandSpec.parameters_schema 一致）
  return_schema: dict[str, Any]
  is_tool: bool                     # 是否同时作为 LLM tool manifest
```

`CommandSpec` 是 `OperationDescriptor` 的命令侧别名/特化，PluginMeta 在装载阶段从 `@command` 装饰的方法**自动**生成 `OperationDescriptor` 并通过 dispatcher 注册（详 §18）。

### 14.2 op_id 命名规范

- **agent-local**：op_id 在同一 Agent 内全局唯一，跨 Agent 不冲突（同进程多 Agent 各持独立 Operation 表）。
- **建议命名**：`<plugin_id>.<method_name>`（来自 @command）或 `<source_namespace>.<op_name>`（来自 `dispatch.register_operation`）。如 `echo.echo`、`todo:default.create`、`qq:bot1.send_msg`。
- **跨 agent 调用**：v0.2 不提供；v0.3 引入显式 `dispatch.invoke_in_agent(agent_id, op_id, payload)`。

### 14.3 注册路径

| 来源 | 注册时机 | 静态声明位置 |
|---|---|---|
| `@command` 装饰的 plugin 方法 | PluginMeta 在 plugin 装载时把 marker 包成 OperationDescriptor，通过 dispatcher 注册 | `Plugin.provides_operations` 由 PluginMeta **自动汇入**（用户无需手写） |
| `dispatch.register_operation(op_id, ...)` | plugin `on_load` 显式调用 | 用户必须在 `Plugin.provides_operations` 显式声明 OperationDescriptor |

### 14.4 静态 vs 运行时校验

- **静态（PluginLoader.discover）**：扫所有 plugin 的 `provides_operations`，构建 `op_id → providing_plugin_id` 反向索引；同名冲突 → `Error(code="operation.conflict")`。
- **DAG 解析（PluginLoader._toposort）**：把 `requires_operations` 转换为 plugin-level 依赖（"A requires op X，X by B → A depends on B"）。
- **运行时（dispatcher.register_operation）**：op_id 必须在调用方 plugin 的 provides_operations 集合内，否则 `Error(code="operation.undeclared")`。
- **dispatcher.invoke 入口**：未注册 → `operation.not_found`；unhealthy → `operation.unhealthy`。

### 14.5 状态机

```text
active → unhealthy（handler 抛出未捕获异常）
unhealthy → active（plugin reload）
active → unregistering（plugin 正在卸载，PluginScope.close 中）
```

dispatcher 提供 `operation_status(op_id) -> OperationStatus` 查询 API。

## 15. Source 协议（v0.2 引入）

> **设计原则**：Source 是「事件推送的标识声明」，与 Operation（被调用的入口）是对称概念。Plugin 主动 publish envelope 时，envelope.source.source_id 必须指向已注册 source_id；ScopeRule 据此做路由匹配。Source 不需要 handler，仅声明元数据。

### 15.1 SourceDescriptor —— 静态声明

```text
SourceDescriptor:
  source_id: str                # agent-local 全限定，如 "qq:bot1" / "todo:default"
  kind: SourceKindName          # 注册式，如 SourceKinds.IM / SourceKinds.TOOL / SourceKinds.HYBRID
  capabilities: tuple[CapabilityName, ...]   # 该源能产生哪类内容（如 Caps.IM_TEXT, Caps.IM_IMAGE）
  description: str
```

### 15.2 SourceKindName

`RegisteredString` 子类 + 内置门面 `SourceKinds`：

- `SourceKinds.IM` —— 即时通讯类（QQ / Discord / Telegram 等）
- `SourceKinds.TOOL` —— MCP 风格软件接口（Todo / 文件系统 / 数据库等）
- `SourceKinds.HYBRID` —— 兼具 IM 与 Tool 性质（如同时收消息又暴露调用面的网关）

领域插件可注册自有 kind：`SourceKindName.register("mcp.fs", declared_by="my-mcp-plugin")`。

### 15.3 注册与校验

- **注册**：`dispatch.register_source(source_id, kind, capabilities)` 仅声明，无 handler。
- **静态声明**：`Plugin.provides_sources: ClassVar[tuple[SourceDescriptor, ...]]`；冲突 → `Error(code="source.conflict")`。
- **运行时校验**：
  - `dispatch.register_source(source_id, ...)` 必须在 plugin.provides_sources 内 → 否则 `source.undeclared`
  - `dispatch.publish(envelope)` 必须 envelope.source.source_id 在已注册集 → 否则 `source.unregistered`

### 15.4 与 Operation 的命名空间共享

Source 与 Operation 共享同一前缀命名约定。例如 QQ 适配 plugin 同时提供：

- Source：`source_id = "qq:bot1"`（kind=IM, capabilities=(Caps.IM_TEXT, Caps.IM_IMAGE)）—— 收到 QQ 消息时 publish envelope
- Operation：`op_id = "qq:bot1.send_msg"` —— 跨 plugin 调用以发送消息

`qq:bot1` 前缀对应"一个 QQ bot 实例"的语义分组。**但它不是独立类型**，仅是命名约定。

## 16. Envelope 协议（v0.2 引入）

> **设计原则**：通用入站/出站载体，IM `Message` 是其特化。adapter 侧的"消息"概念只是 Envelope 的一种 payload schema；MCP 风格 ToolEvent 是另一种。dispatcher 路由按 `payload_schema_id` 与 `source` 决定。

### 16.1 Envelope 基类

```text
Envelope (Contract):
  schema_id: ClassVar[str] = "mutsukibot.envelope"
  schema_version: ClassVar[str] = "1.0.0"
  
  id: EnvelopeId                          # 由 runtime 注入
  timestamp: float                        # 由 runtime 注入
  source: SourceRef                       # 来源标识
  payload_schema_id: str                  # 路由主 key（区分 Message / ToolEvent / 领域 envelope）
  capabilities_required: tuple[CapabilityName, ...] = ()
```

### 16.2 SourceRef 层级

```text
SourceRef (Contract):                     # 通用基类
  schema_id: ClassVar[str] = "mutsukibot.source_ref"
  source_id: str                          # 引用已注册的 SourceDescriptor.source_id
  kind: SourceKindName

ChannelRef (SourceRef):                   # IM 特化（v0.1 已有，v0.2 改为继承 SourceRef）
  schema_id: ClassVar[str] = "mutsukibot.channel_ref"
  channel_id: str
  user_id: str | None = None
  # source_id 继承自 SourceRef，**字段从 v0.1 的 adapter_id 重命名而来**
  # v0.2 保留 `adapter_id` 作为只读 property 一个 release，打 DeprecationWarning

ToolSourceRef (SourceRef):                # 工具型 Source 特化
  schema_id: ClassVar[str] = "mutsukibot.tool_source_ref"
  endpoint_path: str | None = None        # 可选的内部路径（如 "todo:default/items"）
```

### 16.3 Message 与 ToolEvent

```text
Message (Envelope):                       # IM 消息特化（v0.1 已有，v0.2 改为继承 Envelope）
  schema_id: ClassVar[str] = "mutsukibot.message"
  parts: tuple[ContentPart, ...]
  # source 字段类型收窄为 ChannelRef
  # payload_schema_id 默认为 "mutsukibot.message"

ToolEvent (Envelope):                     # MCP 风格事件推送（v0.2 新增）
  schema_id: ClassVar[str] = "mutsukibot.tool_event"
  event_type: str                         # 领域事件类型，如 "todo.created" / "fs.changed"
  payload: dict[str, Any]                 # 领域 payload；复杂场景用 RefPayload[T]
```

### 16.4 路由 key

dispatcher 路由 envelope 时，主匹配键是：
1. `envelope.payload_schema_id`（对应 `BySchema(...)` / `BySchemaPrefix(...)`）
2. `envelope.source.source_id`（对应 `BySourceId(...)`）
3. `envelope.source.kind`（对应 `BySourceKind(...)`）
4. `envelope.capabilities_required`（对应 `ByCapability(...)`）

ScopeRule 谓词组合见 §17。

## 17. ScopeRule 协议（v0.2 引入）

> **设计原则**：完全镜像 [`PermissionRule`](../mutsukibot/contracts/permission.py) —— AST 谓词组合 + 注册式命名。差别仅在：PermissionRule 检查 `AgentContext`（who can call），ScopeRule 检查 `Envelope`（whether to route）。

### 17.1 ScopeRule —— 谓词组合

```text
ScopeRule:
  __and__ -> 合并所有 checker（AND）
  __or__  -> 包装为新 OR 节点
  check(envelope) -> bool          # 同步方法（纯数据匹配，无副作用）
```

实现细节与 PermissionRule 同型：抽象基类 + `_Leaf / _And / _Or` 三个 AST 节点；`__and__` / `__or__` 在组合时平展同类节点。

### 17.2 内置叶子构造器

| 构造器 | 语义 |
|---|---|
| `BySchema(schema_id: str)` | 严格匹配 envelope.payload_schema_id |
| `BySchemaPrefix(prefix: str)` | envelope.payload_schema_id startswith prefix（如 `"yume."`）|
| `BySourceId(source_id: str)` | 匹配 envelope.source.source_id |
| `BySourceKind(kind: SourceKindName)` | 匹配 envelope.source.kind |
| `ByCapability(cap: CapabilityName)` | envelope.capabilities_required 包含 cap |
| `BySourceField(field: str, value: Any)` | envelope.source 任意字段精确匹配（如 `BySourceField("channel_id", "ops")`）|

### 17.3 ScopeName —— 命名 scope 注册式

与 `PermissionName` 同模式：`RegisteredString` 子类 + 内置门面 `Scopes`。核心内置：

- `Scopes.IM_TEXT` = `BySchema("mutsukibot.message") & BySourceKind(SourceKinds.IM) & ByCapability(Caps.IM_TEXT)`
- `Scopes.TOOL_INVOKE` = `BySchema("mutsukibot.tool_event") & BySourceKind(SourceKinds.TOOL)`

插件可注册自有 scope：`ScopeName.register("yume.thought", declared_by="...", rule=...)`。

### 17.4 消费点

| 消费方 | 字段 | 作用 |
|---|---|---|
| `Agent.accepts: tuple[ScopeRule, ...]` | dispatcher 路由 envelope 时筛选目标 Agent | 空 tuple = 拒绝所有 envelope（仅命令路径仍可用，hard rule #13）|
| `Plugin.consumes: ClassVar[tuple[ScopeRule, ...]]` | scheduler 把 envelope 二次分发给 plugin 内 handler | 空 = plugin 不消费 envelope（仅命令型 plugin，如 echo）|
| `@command(consumes=...)` 可选 | 命令级粒度细化 | 默认与 plugin.consumes 等同 |

## 18. Dispatcher 协议（v0.2 引入）

> **设计原则**：dispatcher 是 envelope/Operation 路由的唯一入口；位于 [core/agent.py](../mutsukibot/core/agent.py) 与 plugins 之间，依赖 `contracts`，不被 plugins 直接 import（plugins 仅通过 `ctx.dispatch` 访问）。

### 18.1 注册 API

```text
dispatch.register_operation(op_id, perms_rule, requires_caps, params_schema, handler)
   -> dispose_callback   # 自动 attach 到调用方 PluginScope

dispatch.register_source(source_id, kind, capabilities)
   -> dispose_callback   # 自动 attach 到调用方 PluginScope

dispatch.lookup_operation(name) -> op_id | None
   # 供 scheduler 文本路径解析首词；按 op_id 后缀匹配（"echo" → "echo.echo"）
```

### 18.2 调用 API

```text
dispatch.invoke(op_id: str, payload: dict | None = None) -> Awaitable[Envelope]
   # **inline await 实现**：直接 `await handler(payload)`，不入 asyncio.Queue 不走 gather
   # 这是延迟敏感链路（v0.5+ Yume thought→kernel→runtime）的硬性前提
   # 与 Bus subscribe(..., direct=True) 同等约束（参见 architecture.md §5）

dispatch.publish(envelope: Envelope) -> Awaitable[None]
   # 校验 envelope.source.source_id 在已注册集 → 否则 source.unregistered
   # 默认 deferred 语义（fan-out 给所有 accepts 匹配的 Agent，gather 并发）
   # direct 模式可选（envelope 标 Replayability.full + 订阅者声明 direct）
```

### 18.3 拦截链

`dispatch.invoke` 的执行顺序：

1. lookup op_id → 不存在 → `operation.not_found`
2. status 检查 → unhealthy → `operation.unhealthy`；unregistering → `operation.unavailable`
3. capability 检查（`check_capabilities`）→ 不通过 → `capability.not_declared`
4. permission 检查（`await rule.check(ctx)`）→ 不通过 → `permission.denied`
5. trace span 开始
6. **inline `await handler(payload)`** —— 不进任何异步队列
7. handler 抛出未捕获异常 → 标记 op 为 unhealthy，返 `operation.handler_raised`，**不主动卸载 plugin**（让 plugin 自决）
8. trace span 结束（status: ok / error）

### 18.4 状态查询

```text
dispatch.operation_status(op_id) -> OperationStatus  # active / unhealthy / unregistering / not_found
dispatch.source_status(source_id) -> SourceStatus
dispatch.list_operations() -> tuple[OperationDescriptor, ...]
dispatch.list_sources() -> tuple[SourceDescriptor, ...]
```

dashboard / 审计插件用以上 API 枚举状态。

### 18.5 与 PluginScope 集成

- `register_operation` / `register_source` 内部把对应 `unregister_*` 作为 dispose 回调挂到调用方 PluginScope
- plugin 卸载时 PluginScope.close() 触发反注册回调，dispatcher 状态保持一致
- 卸载流程对照 PluginLoader.unload_from（[loader.py:205](../mutsukibot/core/loader.py#L205)）：on_unload → scope.close → dispatcher 状态自动清理

### 18.6 Trace span（v0.3 后续）

- `dispatch.invoke` 必须发出 `TraceSpan(name="dispatch.invoke")`，attributes 至少包含 `agent_id` / `op_id` / `plugin_id`。
- `dispatch.invoke_in_agent` 必须发出 `TraceSpan(name="dispatch.invoke_in_agent")`，attributes 至少包含 `agent_id` / `target_agent_id` / `op_id`。
- 跨 Agent 调用时，目标 Agent 的 `dispatch.invoke` span 必须沿用调用方 `trace_id`，并以调用方 `dispatch.invoke_in_agent` span 为 parent，保持跨 bus 的同一 trace 因果链。
- span 通过 `ctx.bus.publish("trace.span", span)` 发出；observability 只旁路订阅，不进入 core 依赖图。

### 18.7 Trace JSONL 与回放 kit（v0.3.2）

- `JsonlTraceWriter` 是 observability 旁路订阅者，只负责把 `trace.span` 事件逐行写为 JSONL。
- `JsonlTraceReader` 按相同格式读回 `TraceSpan`；记录缺字段、字段类型错误或非法枚举值时抛结构化 `Error(code="trace.record_invalid")`。
- `mutsukibot.testing.replay_trace_spans(...)` 只验证已记录的 span 因果链，不重放外部副作用；输出 `TraceReplayFrame`，包含 `span` / `depth` / `parent_span_id` 等测试断言用信息。
- replay kit 拒绝同一 trace 内重复 `span_id`、非法时间区间、父链环；当测试要求闭合父链时，缺失 parent 也必须 fail-loud 为 `trace.replay_failed`。
- 默认允许父 span 不在当前文件内，以支持跨 Agent / 分片 trace 文件；契约测试可显式开启 `require_known_parents=True`。

## 19. Agent Election Policy（v0.3.3）

`AgentRegistry` 负责多 Agent 路由候选筛选与单 winner 选择。策略插件只能替换
候选排序，不得绕过 Agent 生命周期与 `accepts` 过滤。

### 19.1 策略接口

```text
AgentElectionPolicy.rank(envelope, candidates) -> tuple[Agent, ...]
```

规则：

- registry 先筛掉非 `LifecyclePhase.AWAKE` Agent 与 `accepts` 不匹配 Agent。
- policy 只接收已匹配候选；默认策略是 `priority` 降序，平手按 `agent_id` 升序。
- `select_accepting(envelope)` 取排序后的第一个候选；`iter_accepting(envelope)` 按同一排序广播。

### 19.2 插件安装

插件通过 `AgentRegistry.install_election_policy(policy, owner=plugin_id)` 安装策略；
返回的 disposer 必须挂到 `PluginScope.add_dispose(...)`。后安装策略优先；卸载当前
策略后恢复上一策略，全部卸载后恢复默认策略。
