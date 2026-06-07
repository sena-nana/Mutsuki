# MutsukiBot 架构设计（v0.0）

本文件回答：**MutsukiBot 是什么、不是什么、为何这样分层、与 Yume / mind-sim 的关系**。具体技术栈与目录结构见 [engineering.md](engineering.md)，契约形态见 [contracts.md](contracts.md)。

## 1. 项目方向

MutsukiBot 是 Agent 中心的 Bot 框架，**不是** 「事件总线 + 命令处理器」式的传统 Bot。设计目的双重：

1. 为 Yume 与 mind-sim 提供运行核心。
2. 通过插件组合实现传统 Bot 框架能力。

Yume / mind-sim 自身的实现路径也将被解构为 MutsukiBot 之上的**零散插件**。它们既是目标用例，也是目标插件生态。

### 借鉴对象与边界

| 来源 | 采用的心智 | 不采用 |
|---|---|---|
| Koishi | 插件系统、服务注入、上下文链式 API、热插拔 | TS API 形态 |
| NoneBot | async 优先、Matcher 优先级、Driver / Adapter 双层、依赖注入 | 全局事件分发模型（在 Agent 中心下需重塑）|
| AstrBot | 多 LLM Provider、多消息平台、Agent / Tool 编排 | 进入核心 —— 仅作为目标插件组的形态参考 |

每个机制必须能解释自己对「Agent 一等公民、解耦、可扩展」中至少一项的贡献。**不允许引入「为像传统 Bot 而像传统 Bot」的机制**。

## 2. Agent 一等公民

每个 Agent 拥有：

- 身份 ID
- Context（运行期上下文）
- 生命周期阶段：`spawn / awake / sleep / stop`
- 独立调度循环
- 消息收件箱
- 可挂载的能力插件

约束：

- Agent **不等于** 「会话」，也 **不等于** 「LLM 调用」。会话是 Agent 的视图；LLM 是 Agent 可调用的 runtime 能力之一。
- Agent 的调度循环必须能容纳 Yume 风格的「持续内部认知」，而不仅是「请求—响应」。
- 多 Agent 必须能在同一进程内共存、隔离、并发；身份与状态不得互相污染。

## 3. 分层

依赖方向（实线 `→` 与 `↑` 表示 import 依赖；虚线 `╌╌` 表示旁路订阅，不构成依赖）：

```text
plugins → core → contracts
            ↑
      runtime（横向支撑 core 与 plugins）

      observability ╌╌> (订阅 core / plugins / runtime 的事件与 trace)
```

`observability` 不被任何层依赖，也不依赖任何层的内部实现，只通过事件总线 / trace 通道旁路订阅。把 observability 从依赖图中剥离是「卸载 observability 不影响主链路」的前提。

**v0.2 关键变更**：删除独立 Adapter 抽象。原"协议适配层"的职责由 reference plugin 承担：plugin 通过 `dispatch.register_source(...)` + `dispatch.register_operation(...)` 暴露与外界的连接。详 [contracts.md §14-§18](contracts.md)。

各层职责（v0.2）：

- `mutsukibot/contracts` —— 稳定内部协议。详见 [contracts.md](contracts.md)。
- `mutsukibot/core` —— Agent 运行时本体：注册中心、调度器、Context 工厂、服务容器、生命周期编排、插件 DAG 加载、事务原语、**Dispatcher**（Operation/Source 注册 + envelope 路由）。
- `mutsukibot/runtime` —— 事件循环策略、并发控制、进程/线程隔离、资源 quota、决定性时间与 ID 源。**Runtime 不决定 Agent 行为**。
- 独立 Adapter 层 —— **v0.2 删除**。原 IM/平台 SDK 适配职责由 reference plugin（`mutsukibot/plugins/inmemory_endpoint/` 等）通过 dispatcher 注册 Source + Operation 实现。
- `mutsukibot/plugins` —— 所有可装可卸的能力（命令、Matcher、记忆、情感、睡眠、LLM 桥接、Yume 模块、**transport reference plugins**）。
- `mutsukibot/services` —— 跨插件共享的具名服务，参考 Koishi 服务注入。服务必须有契约。
- `mutsukibot/observability` —— trace、audit、metrics、事件总线观测。
- `mutsukibot/common` —— 纯工具，禁止承载业务逻辑。

边界约束：

- `core` 不依赖任何具体 plugin / runtime 后端实现。
- `plugins` 之间禁止直接 import 兄弟插件实现模块，只能通过契约 + 服务 + Operation 通信。
- 外部协议定义（OneBot / MCP / ChatCompletion 等）只能出现在 reference plugin 中，不得渗入 `core` / `contracts`。
- 反模式：把「LLM 调用 / 记忆 / 情感」做进 `core` —— 这是 v1 mind-sim 的教训之一。

## 4. 与 Yume / mind-sim 的关系

Yume 与 mind-sim **既是目标用例，也是目标插件生态**。其原本的紧耦合实现路径将被解构为多个 MutsukiBot 插件，按契约组合复现原设计意图。

MutsukiBot **不在源码层 import** Yume / mind-sim。二者作为插件组存在，命名空间建议：

- `mutsukibot-contracts-yume` —— 共享契约包（thought / kernel / sleep）
- `mutsukibot-yume-architecture` —— consciousness loop / stimulus / thought engine / expression
- `mutsukibot-yume-kernel` —— router + text / latent / skill / reflect kernels
- `mutsukibot-yume-runtime` —— vLLM / KV cache / latent bridge / training
- `mutsukibot-yume-evolution` —— sleep collector / evaluator / compiler / integrator
- `mutsukibot-yume-memory` / `-affect` / `-identity` / `-skills`
- `mutsukibot-mindsim-*` —— 同模式拆解 mind-sim 的 `bus` / `engine` / `provider` / `server`

**承载性测试样本**：设计任何 MutsukiBot 核心契约时，必须用 Yume 的 `StimulusEvent / ThoughtPacket / KernelRequest / ExpressionDecision / SleepCandidate` 与 mind-sim 的 `bus / engine / provider` 作为测试样本。契约若无法承载这些类型则**修契约**，不为任一系统特化。

参考阅读（**禁止复制代码**）：

- [Yume/yume/](../../../Yume/yume/)
- [Yume/mind-sim/mind_sim/](../../../Yume/mind-sim/mind_sim/)

## 5. 拆解风险与预埋接口

把紧耦合的 Yume / mind-sim 拆为零散插件存在固有风险。v0.0 contracts 必须预埋以下机制，否则 v1+ 只能推倒重写：

| 风险 | 来源 | v0.0 预埋机制 |
|---|---|---|
| 非序列化对象跨插件传递（含 latent / KV cache） | Yume `latent_bridge` / `kv_cache` 持 GPU 内存；其他插件可能有会话句柄、设备句柄等 | 服务注入 `by-ref` 语义（长生命周期）+ 通用 `RefPayload` / `Handle` / `RefDescriptor` 协议（瞬态调用间）；详见 [contracts.md §11](contracts.md#11-通用-by-ref-payload-协议) |
| GPU / VRAM / token 资源争抢 | Yume runtime 多组件共享 GPU | Capability 资源量化 + ResourceGovernor 契约 |
| sub-ms 延迟链路 | `thought → kernel → runtime` | 事件总线 `direct-dispatch` 快路径 |
| Sleep 多步事务一致性 | `collect → evaluate → compile → integrate → rollback` | Saga / TransactionScope 原语 |
| 多插件共享类型 | `ThoughtPacket` / `KernelRequest` | contracts-only 共享包机制 |
| 启动顺序刚性 | `runtime → kernel → architecture → transport plugins` | 插件依赖 DAG + 拓扑加载 |
| 睡眠期身份连续性 | transport / patch 版本切换 | Agent 状态 `snapshot` + `atomic_swap` |
| 复杂嵌套配置 | latent / KV / sleep 配置树 | 配置 schema 支持 union / 版本 / 引用 |
| 跨插件因果调试 | 拆插件后丢失调用栈 | Trace 强制带 `trace_id` / `span_id` 因果链 |
| 测试面爆炸 | N 插件 ⇒ N² 集成路径 | Contract test kit 可复用 |
| 多 transport 协调（IM + MCP + 软件 API 同时流转） | 原 Adapter 1:1 设计无法表达 N:M 路由 | Dispatcher + Envelope `payload_schema_id` 路由 + Agent.accepts ScopeRule（[contracts.md §14-§18](contracts.md)） |
| transport 翻译职责泛滥 | 原 Adapter 类型把"协议翻译"做成独立机制，与 Plugin 重复 90% | 复用 plugin 机制（Endpoint 仅是 plugin 的注册行为，无独立类型；Operation/Source 注册自动 attach 到 PluginScope）|

这些预埋只在 contracts / 设计文档中体现，**不写实现**。

**验证标准**：拿 Yume v0.4 的一个实际 thought tick 走查上面 10 条，每条都能找到契约支撑点。

**反向论证（红线）**：若未来出现「必须把 latent handle 序列化才能跨插件传」、「必须让全部消息走异步队列」、「必须让 sleep 流程通过松耦合事件链表达」这类需求，应**修 MutsukiBot 契约**，而不是把能力塞回 Yume 内部。这是判定 MutsukiBot 设计是否还在正轨的指针。

## 6. Generic By-Ref vs Domain-Specific Use（核心如何保持领域中立）

MutsukiBot 核心面对一个看似矛盾的需求：

- 必须支持「插件之间传递非序列化对象」（否则 Yume 的 latent / KV cache 路径走不通）。
- 但**核心不能假设**这些对象是 latent、是张量、是 GPU 句柄——否则就把领域语义焊进了核心，违反 [§1](#1-项目方向) 的反模式。

解决方案：**核心提供通用机制，领域语义由契约包定义**。

### 6.1 核心层只知道什么

- 「这是一个按引用传递的字段」（`RefPayload[T]`）
- 「这个引用有所有者，需要被释放」（`Handle[T]`）
- 「这个引用有可观测的元数据」（`RefDescriptor`，含 `kind / schema_id / attributes`，但 `attributes` 内容核心不解读）
- 「这种引用不能跨进程 / 跨域」（装载校验 + 序列化拒绝）
- 「这条流可能不可重放」（`Replayability` 声明）

### 6.2 核心层永远不知道什么

- 这是不是张量、是不是 latent、是不是 KV cache
- `attributes` 里 `shape / dtype / device / model_version` 是什么意思
- 哪些版本之间兼容（兼容回调由契约包注册）
- 引用的具体清理逻辑（finalizer 由创建者注册）

### 6.3 领域插件做什么

`mutsukibot-contracts-yume`（举例）：

- 定义 `LatentRef = RefPayload[torch.Tensor]`，并约定 `attributes` 包含 `shape / dtype / device / model_version / latent_space_id`。
- 定义 `KVCacheRef = RefPayload[VLLMKVHandle]`，约定 `attributes` 包含 `slot_id / model_version / page_count`。
- 注册 schema 兼容回调：`is_compatible("yume.latent/v1.2", "yume.latent/v1.3") -> bool`。
- 申报领域 capability：`vram` / `kv_slot`（核心只见过通用的 `hold_ref` / `borrow_ref` / `produce_ref_stream`）。
- 实现 finalizer：释放 GPU 内存、归还 KV slot。

mind-sim、其他第三方插件同理。

### 6.4 验证标准

判断核心是否还在「领域中立」轨道上：

- ✅ 核心代码搜不到 `latent` / `kv` / `tensor` / `gpu` 字样。
- ✅ 核心 `Capability` 已知能力列表里没有 `vram` / `kv_slot`（这些只在 yume 契约包出现）。
- ✅ 把所有 yume 插件 / 契约包卸载后，核心仍能跑通传统 Bot 工作流。
- ❌ 若发现核心需要新增 `LatentRef` 类型或 `latent_space` 字段，**必须改回通用 `RefPayload` + 领域 attributes**，不在核心特化。

## 7. 数据流参考

### 7.1 传统 Bot 路径

```text
External Input
  → transport plugin
  → Message
  → Agent inbox
  → Matcher / Command
  → Tool/Service call
  → transport Operation response
```

### 7.2 Yume 风格清醒路径（通过插件组合实现）

```text
External Input
  → transport plugin
  → StimulusEvent（来自 mutsukibot-contracts-yume）
  → StimulusSystem 插件
  → ThoughtEngine 插件
  → ConsciousnessLoop（Agent 调度循环）
  → KernelRequest → Kernel 插件
  → KernelResult
  → Expression / Skill / Internal Update
  → Memory + Affect + Identity 插件更新
```

### 7.3 睡眠路径（事务化）

```text
Awake Traces
  → SleepCollector 插件
  → SleepEvaluator 插件
  → SleepCompiler 插件
  → SleepArtifact
  → SleepIntegrator 插件（在 TransactionScope 内）
  → VersionRegistry 服务
  → Agent atomic_swap → Updated future cognition
```

### 7.4 外部后端事件桥接路径（当前边界）

```text
External Backend Event
  → bridge plugin 后台 task（attach 到 PluginScope）
  → bridge plugin 定义 BackendEvent / BackendSourceRef / SourceKindName("example.backend")
  → ctx.dispatch.publish(BackendEvent(payload_schema_id="example.backend.item_changed",
                                      source=BackendSourceRef(source_id="backend:default")))
  → Dispatcher
  → 匹配 Agent.accepts (ScopeRule，如 BySchemaPrefix("example.backend."))
  → Agent inbox
  → 匹配 plugin.consumes
  → plugin handler
  → ctx.dispatch.invoke("backend:default.notify", {...})  # Agent 表达行动
  → bridge plugin handler
  → external backend
```

关键性质：
- **dispatch.invoke 是 inline await**（参见 [contracts.md §18.2](contracts.md) 与 §5 sub-ms 风险），保 v0.5+ Yume `thought → kernel → runtime` 链路不被异步队列拖慢
- **Agent 易识别在操作哪个外部能力**：op_id 字面量（如 `"qq:bot1.send_msg"` / `"backend:default.notify"`）在调用现场可见
- **Core 不持有业务数据权威**：真实 todo / 文件 / 应用状态属于外部后端或领域插件；Core 只接收事件、路由给 Agent，并通过 Operation 表达 Agent 可采取的动作
- 资源（socket / SDK client）走 [`Handle[T]`](contracts.md#11-通用-by-ref-payload-协议) attach 到 PluginScope（hard rule #14）—— plugin 卸载时 finalizer 自动关闭
