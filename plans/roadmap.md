# MutsukiBot 路线图

本文件回答：**当前在哪个版本、做什么、不做什么、何时进入下一版本**。

## 当前边界：Agent 事件行动核

**目标**：Core 收束为 Agent 运行核心：接收外部后端或协议桥转换后的 Envelope，驱动 Agent 决策，并通过 Operation 表达可采取的动作。Core 保留 Plugin + 注册式 Operation/Source/Dispatcher，但不内置应用后端 / CRUD endpoint / tool event 语义。历史 v0.2 已完成；产出报告见 [version-reports/v0.2.md](version-reports/v0.2.md)。

## 历史版本：v0.0 骨架

**目标**：建立项目宪法、分层、契约草案与规则文档，为后续实现提供唯一事实来源。**不写实现代码**。

### v0.0 范围（In Scope）

| 文件 | 状态 |
|---|---|
| `AGENTS.md` | 项目宪法 + 索引 |
| `README.md` | 一句话定位 |
| `plans/roadmap.md` | 本文件 |
| `plans/architecture.md` | 方向、Agent 一等公民、分层、与 Yume / mind-sim 关系、拆解风险 |
| `plans/engineering.md` | 技术栈、目录、插件模型、横切公约实现层规则、测试基础设施 |
| `plans/contracts.md` | 内部协议草案（核心契约对象骨架） |
| `pyproject.toml` | 最小依赖与工具链配置 |

### v0.0 不做（Out of Scope）

- 任何实现代码（`mutsukibot/` 目录暂不创建）
- LLM provider 集成
- 任何具体消息平台 transport plugin（OneBot / QQ / Discord / Telegram 等）
- 持久化层
- Web 控制面板
- 国际化
- 性能基准
- Yume / mind-sim 任何插件的实现

### v0.0 验收标准

任意新协作者读完 `AGENTS.md + plans/*` 能复述：

1. MutsukiBot 是什么 / 不是什么
2. 与 Koishi / NoneBot / AstrBot 的借鉴边界
3. 与 Yume / mind-sim 的关系
4. Yume / mind-sim 为何能拆插件，以及拆解的风险与对策
5. 下一步做什么（v0.1 范围）

文档自身不包含实现代码或 API 形态描述（避免锁死）。

## 下一版本：v0.1 最小可运行骨架

**目标**：第一个可装载、可运行、可被测试的 Agent + 一个回声插件 + 一个 in-memory transport reference plugin。

### v0.1 候选范围

- `mutsukibot/contracts/` 锁定 v0.1 字段（在 [contracts.md](contracts.md) 草案上加版本字段），含通用 by-ref 协议骨架：`RefPayload[T]` / `Handle[T]` / `RefDescriptor` / `BackpressureChannel[T]` / `Replayability` 声明
- `mutsukibot/core/`：
  - 注册中心（Agent / Plugin / Service registry）
  - 调度器（最小 `tick` 循环）
  - Context 工厂
  - 服务容器（支持 by-value / by-ref）
  - 插件 DAG 加载器
  - `PluginScope` 与 `TransactionScope`，含 `Handle` 自动释放与泄漏检测
- `mutsukibot/runtime/`：
  - 决定性时间与 ID 源
  - 事件循环包装
  - 同步点检查
- `mutsukibot/plugins/`：
  - In-memory transport reference plugin（测试基础设施）
- `mutsukibot/plugins/`：
  - 一个 echo 命令插件（同时是 LLM tool，验证「指令即工具」hard rule）
- `mutsukibot/observability/`：
  - 结构化 trace 写入器（含因果链）
- `tests/`：
  - 基线契约测试套件
  - echo 插件冒烟测试
  - 热重载测试（验证 `PluginScope` 完整回收）
  - by-ref 协议测试：用 stub `Handle` 验证瞬态引用在 ≥2 插件间通过 `RefPayload` 传递、scope 关闭时自动释放、序列化 / 跨域时正确报错

### v0.1 门控

- 一个 Agent 能 spawn → awake → 处理一条 echo → sleep → stop
- echo 插件能被人类触发，也能作为 LLM tool 被调用
- 热重载 echo 插件 100 次后无资源泄漏
- 所有横切公约 lint 规则就位
- Yume v0.4 的某个 `StimulusEvent → ExpressionDecision` 样本能用 v0.1 契约表达（即使没有 Yume 插件实现，也要能序列化 / 反序列化通过）
- 通用 by-ref 协议自洽：用 stub 引用模拟一条「插件 A 产生 ref → 插件 B 借用 ref → scope 关闭自动释放」链路，全程核心代码不出现任何领域字样

## 已完成版本：v0.2 通用 Agent 框架改造

**已完成**（Phase A + Phase B + Phase C）—— 把 Adapter 抽象拆解为 Plugin + 注册式 Operation/Source（Option IV，详见
[contracts.md §14-§18](contracts.md) 与 plan 文档 D1/D9/D9b/D12/D13）：

- 契约层：`Envelope` / `SourceRef` / `Operation` / `Source` / `ScopeRule` / `Dispatcher` 五节协议
- Hard Rule #13（accepts 显式拒绝）+ #14（I/O 资源外置）
- Dispatcher 实现：register_operation / register_source / lookup_operation / invoke (inline await) / publish 广播筛选
- D9b：`provides_operations` / `provides_sources` / `requires_operations` / `requires_sources` 静态声明 + DAG 拓扑 + dispatcher undeclared 运行时校验
- D12：`@command` 与 Operation 统一，scheduler 命令路径走 dispatcher
- envelope 二次分发：`Plugin.consumes` ScopeRule + `on_envelope` hook
- Reference 插件：`InMemoryEndpointPlugin` / OneBot reference plugin；旧 `TodoPlugin` / `QqToTodoPlugin` 后端化样板已从 Core 包裁剪，迁移为外部后端桥接示例。
- 测试：130 通过（v0.1 74 + Phase A 36 + Phase B 20）
- 当前 smoke 端到端：`echo` / `cross_agent` / OneBot reference 测试；外部后端协作通过自定义 SourceKind / Envelope / Operation 测试 fixture 覆盖。
- Phase C 多 Agent 广播：
  - `mutsukibot/core/agent_registry.py`：进程全局弱引用 `AgentRegistry`
  - `Agent.__post_init__` 自动注册；`Dispatcher.publish()` 广播给所有 awake 且 `accepts` 匹配的 Agent
  - `mutsukibot/plugins/cross_agent_smoke.py` 验证 control / audit 双 Agent 同收 IM envelope
- 配置 schema 自动校验：
  - `PluginLoader.load_into(..., configs=Mapping[str, object])` 接受原始 mapping / payload
  - 装载前用 `msgspec.convert(..., type=cls.Config)` 转换并校验
  - 配置错误 fail-loud 为 `plugin.config_invalid`，外层仍包装成 `PluginLoadFailedError`

**v0.2 收尾完成项**：

- OneBot v11 反向 WebSocket reference plugin：只在 plugin 内处理 OneBot 外部协议，Source/Operation 通过 dispatcher 暴露，server/connection 走 `Handle` + `PluginScope`。
- hard rule #14 lint：扫描 Plugin 子类字段，拒绝裸持 raw socket / SDK client / websocket server/connection。
- dispatcher.invoke microbenchmark gate：建立 sub-ms 早期基线。
- Operation/Source 反注册 contract helper：复用断言 plugin 卸载后 dispatcher 无残留。
- docs 同步：adapter 文档迁移为 transport plugin / endpoint / dispatcher 文档。

## 下一阶段：v0.3 MVP 多 Agent 与资源协商

**目标**：在 v0.2 Dispatcher / Source / Operation 基础上补齐最小可用的多 Agent
协作与资源生命周期能力，作为后续 Yume / mind-sim 插件拆解的运行时底座。

### v0.3 MVP 范围

- `dispatch.invoke_in_agent(agent_id, op_id, payload)`：显式跨 Agent 调用，仍保持
  inline await，不经队列。
- `AgentRegistry` 增加确定性候选选择：按 priority + agent_id 选择可接收某
  envelope 的目标 Agent，为后续优先级 / 选举策略留接口。
- `ResourceHost`：进程内资源托管服务，返回 `Handle[T]`，资源可跨 plugin reload
  存活，租约归还后再释放物理资源。
- 最小资源协商：按 `CapabilityName` 声明容量，`acquire/release` 返回可观测租约；
  超额 fail-loud 为结构化 `capability.exhausted`。
- `Saga` 扩展：主链失败 + 补偿失败时携带结构化 `Error(code="transaction.compensation_failed")`。

### v0.3 MVP 不做

- 不做分布式 ResourceHost、不做跨进程资源迁移。
- 不做复杂选举算法；v0.3 只提供确定性排序与显式 winner。
- 不做类型化 Handle 自动注入的完整实现；保留为 v0.3 后续或 v0.4。

### v0.3 MVP 验收

- 跨 Agent 调用能调用另一个 Agent 已注册 Operation，目标不存在 / op 不存在均结构化失败。
- ResourceHost 创建的 Handle 可在 plugin scope 卸载后继续由 host 持有，host 关闭时释放。
- 资源租约超额时返回 `capability.exhausted`，释放后可再次获取。
- Agent 选举在相同输入下稳定，priority 高者胜出，平手按 agent_id。
- `pytest`、`ruff`、`pyright`、`pyrefly` 与现有 smoke 均通过。

### v0.3 后续一：资源注入与 trace（已完成）

- `Dependent` 支持 `Annotated[Handle[T], RefArg(...)]` 类型化注入。
- `RefArg(source=payload)` 校验 payload 中的 `Handle` / `RefPayload` 与声明 kind 一致。
- `RefArg(source=resource_host)` 通过 `ResourceHost` 服务按 `ref_id` 解析句柄，并写 `resource_host.get_handle` span。
- `ResourceHost` 增加句柄索引、`ResourceRecord`、可插拔 eviction / keepalive policy。
- `ResourceHost.acquire_for` / `release_for` 与 `dispatch.invoke` / `invoke_in_agent` 发出 trace span，跨 Agent 调用保持 parent-child 因果链。
- 阶段报告见 [version-reports/v0.3.1.md](version-reports/v0.3.1.md)。

### v0.3 后续二：trace 回放闭环（已完成）

- `JsonlTraceWriter` / `JsonlTraceReader` 形成同构 JSONL trace 记录链路。
- `mutsukibot.testing.replay_trace_spans(...)` 作为可复用契约测试 kit，校验重复 span、父链闭包、非法时间区间和确定性排序。
- 结构化错误码补齐：`trace.record_invalid` / `trace.replay_failed`。
- 阶段报告见 [version-reports/v0.3.2.md](version-reports/v0.3.2.md)。

### v0.3 后续三：选举策略插件化（已完成）

- `AgentElectionPolicy` 把固定 `priority + agent_id` 选择提升为可替换策略。
- `AgentRegistry.install_election_policy(...)` 支持插件安装策略并返回 scope disposer。
- registry 仍固定执行 lifecycle + `Agent.accepts` 过滤，策略只排序候选。
- 阶段报告见 [version-reports/v0.3.3.md](version-reports/v0.3.3.md)。

### v0.3 后续四：ResourceHost 策略参数治理（已完成）

- `ResourceHostPolicyConfig` / `ResourceRecordSelector` 作为治理配置契约，统一描述
  eviction / keepalive 的选择条件。
- `ResourceHost` 接收配置化策略并进行严格校验：拒绝未知字段、空 selector 与
  显式 config/callable 冲突。
- 阶段报告见 [version-reports/v0.3.4.md](version-reports/v0.3.4.md)。

### v0.4：Contract test kit 与跨插件因果 trace（已完成）

- `mutsukibot.testing.contract_kit` 提供可复用的契约断言入口：
  `assert_trace_tree_closed`、`assert_cross_agent_trace_chain`、`assert_dispatcher_clean`。
- `tests/core/test_dispatcher_cross_agent.py` 直接复用 kit 断言跨 Agent trace 链，
  形成可迁移的因果闭环样本。
- 阶段报告见 [version-reports/v0.4.md](version-reports/v0.4.md)。

## 后续版本（仅方向，不锁字段）

| 版本 | 主题 |
|---|---|
| v0.3 后续 | ResourceHost 策略参数治理（已完成） |
| v0.4 | Contract test kit、跨插件因果 trace 完整闭环（已完成） |
| v0.5 | 第一个 Yume 插件落地（`mutsukibot-yume-architecture` + `mutsukibot-yume-kernel` 文本模式）；门控含「latent / 任意非序列化引用在 ≥2 插件间通过通用 `RefPayload` 协议传递，核心代码与 trace 字段中不出现 `latent` / `tensor` / `gpu` 字样」 |
| v0.6 | LLM 桥接插件（多 Provider）、`mutsukibot-yume-runtime` 文本推理 |
| v0.7 | `mutsukibot-yume-evolution` 睡眠插件（事务化） |
| v0.8 | mind-sim 插件首批落地 |
| v0.9 | Web 控制面板插件、配置面板自动生成 |
| v1.0 | 完整 Yume v0.4 行为可在 MutsukiBot 上复现，文档冻结 |

每个 v0.x 完成时产出 `plans/version-reports/v0.x.md`：方向、完成项、基线、运行检查、效果检查、下版门槛。

## 反向论证（红线）

若任一版本出现以下需求，应**修 MutsukiBot 契约**而不是把能力塞回 Yume / mind-sim 内部：

- 必须把 latent handle 序列化才能跨插件传
- 必须让全部消息走异步队列
- 必须让 sleep 流程通过松耦合事件链表达
- 必须让插件直接 import 兄弟插件实现模块
- 必须在 `core` 中内置某个业务概念（LLM / 记忆 / 情感等）

这是判定 MutsukiBot 设计是否还在正轨的指针。

## Plan 同步规则

- 代码即事实，plans 是契约 + 决策。
- 公共契约 / 插件协议 / 生命周期阶段 / 服务接口变化 → 同 PR 内更新 `plans/`。
- plans 保持精简，过期讨论删除，但接口契约与决策必须保留。
