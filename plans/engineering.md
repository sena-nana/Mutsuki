# MutsukiBot 工程实现规则（v0.0）

本文件回答：**用什么栈、目录怎么组织、插件怎么写、横切公约的实现层规则、测试如何展开**。架构原因见 [architecture.md](architecture.md)，契约形态见 [contracts.md](contracts.md)。

## 1. 技术栈

- **Python 3.13**
- **uv** —— 包与虚拟环境管理
- **asyncio** —— 默认异步运行时；可选 `anyio` 抽象
- **msgspec** —— 序列化与配置 schema（性能 + 强类型 + 自动校验）
- **typer** —— CLI adapter（v0.1 后）
- **rich** —— 终端输出
- **pyyaml** —— 配置文件加载
- **pytest** + **pytest-asyncio** —— 测试
- **ruff** + **pyright** + **pyrefly** —— lint 与双类型检查器（pyright `standard` 模式 + Meta pyrefly 严格变性检查；CI 必须两者都通过）

**禁止**：在 v0.0 阶段引入除上述以外的运行时依赖。任何新依赖须在 PR 中说明对核心目标的贡献。

## 2. 目录结构

```text
MutsukiBot/
  AGENTS.md                  # 项目宪法 + 索引
  README.md                  # 一句话定位
  LICENSE
  pyproject.toml
  uv.lock
  config/
    default.yaml
  plans/
    roadmap.md
    architecture.md
    engineering.md           # 本文件
    contracts.md
    version-reports/
      v0.0.md
  tests/
  mutsukibot/
    __init__.py
    contracts/               # 仅类型定义，无运行时副作用
    core/
    runtime/
    adapters/
    plugins/                 # 一等公民插件（核心仓库内可有 reference 实现，v0.1 起）
    services/
    observability/
    common/
```

> v0.0 仅产出 `AGENTS.md`、`README.md`、`plans/*`、`pyproject.toml`。`mutsukibot/` 与 `tests/` 在 v0.1 引入第一个可运行模块时再创建。

## 3. 插件模型

### 3.1 插件定义

每个插件 = **manifest** + **装载点** + **卸载点**。

manifest 必含字段：

- `id`（kebab-case，如 `mutsukibot-yume-architecture`）
- `version`（SemVer）
- `contracts`（依赖的契约包及版本范围）
- `capabilities`（申报的能力，含资源量纲）
- `provides_services`（提供的服务名 + 契约引用）
- `requires_services`（依赖的服务名 + 契约引用）
- `requires_plugins`（依赖的插件 id + 版本范围，用于 DAG 拓扑）
- `config_schema`（msgspec.Struct 类型引用）
- **v0.2 新增**：
  - `consumes`：tuple[ScopeRule, ...]，声明插件消费哪些 envelope（缺省 = 不消费，仅命令型 plugin）
  - `provides_operations`：tuple[OperationDescriptor, ...]，静态声明会注册到 dispatcher 的 Operation（@command 装饰的方法由 PluginMeta 自动汇入）
  - `provides_sources`：tuple[SourceDescriptor, ...]，静态声明会注册的 Source
  - `requires_operations`：tuple[OperationDep, ...]，依赖的外部 Operation（用于 DAG 反向解析）
  - `requires_sources`：tuple[SourceDep, ...]，依赖的外部 Source

### 3.2 PluginScope（无副作用热重载 hard rule）

core 提供 `PluginScope` 用于追踪所有可清理资源。卸载时 scope 自动回收：

1. 事件 / 消息订阅
2. 定时器 / 周期任务
3. 服务注册
4. Agent 上下文挂件
5. 配置监听

**未通过 scope 注册的副作用即视为违规**。Lint 规则（v0.1 起）将检查直接调用全局 `loop.create_task`、`asyncio.get_event_loop()`、`bus.subscribe(...)` 不带 scope 参数等模式。

### 3.3 指令即工具（hard rule）

插件命令通过 `@command` 装饰器声明，框架基于以下信息自动生成「人类可触发命令」与「Agent / LLM 可调用工具」的双重 manifest（合并规则 v0.1）：

| 信息 | 来源（按优先级） |
|---|---|
| 命令/工具 description | docstring 首段 |
| 参数 description | docstring `Args:` 段（Google 风格，由 `docstring_parser` 解析）→ fallback `Annotated[T, Arg(desc=...)]` |
| 参数类型 | 函数签名类型注解 |
| 参数约束 (`ge` / `le` / `min_length` / `max_length` / `regex` / `choices`) | `Annotated[T, Arg(...)]` |
| 默认值 | 函数签名默认值 |

签名解析与 schema 合成实现：[mutsukibot/core/dependency.py](../mutsukibot/core/dependency.py)
（`Dependent` + `Param` ABC）与 [mutsukibot/core/plugin.py](../mutsukibot/core/plugin.py)
（`_build_command_spec`）。两套调用路径（人类命令 + LLM tool）共用同一个实现函数。**禁止维护两份**。

### 3.4 服务注入

服务可声明为：

- `by-value` —— 可序列化、可跨进程
- `by-ref` —— 同进程对象引用、零拷贝（用于 GPU 句柄、KV cache、模型实例等）

注入点支持构造时注入与方法级注入两种风格（参考 NoneBot 依赖注入）。

## 4. 横切公约（实现层规则）

> 公约的设计动机见 [architecture.md §5](architecture.md#5-拆解风险与预埋接口)。本节聚焦**实现层规则**。

### 4.1 Capability 申报（资源化）

- **v0.1 实现**：`CapabilityName` 注册式 + `Caps` 内置门面。所有 manifest 申报与命令 `requires_capabilities` 必须使用 `Caps.READ_MESSAGE` 等常量；裸字符串 `Capability("read_message")` 在构造时即拒。详见 [contracts.md §4](contracts.md#4-capability-命名)。
- 已知能力名（v0.0 草案）：`read_message` / `send_message` / `call_llm` / `persist` / `network_egress` / `spawn_agent` / `hold_ref` / `borrow_ref` / `produce_ref_stream`。后三项是通用 By-Ref 协议（[contracts.md §11](contracts.md#11-通用-by-ref-payload-协议)）的强制 capability。领域能力（`yume.vram` 等）由领域契约包注册，不在核心。
- 量纲示例：`Capability(name=Caps.CALL_LLM, quantity={"tokens_per_min": 60000})`。
- **未申报即调用视为违规**。core 提供 [`check_capabilities`](../mutsukibot/core/capability_guard.py) 在调用入口检查。

### 4.2 快路径直调

事件总线提供 `direct-dispatch` 选项：

- 仅限同进程、同信任域、声明过的订阅。
- 订阅者以同步 awaitable 直调，不进入异步队列。
- 用于延迟敏感链路（如 `thought → kernel → runtime`）。

### 4.3 事务 / 补偿原语

core 提供：

- `Saga` —— 多步骤事务，每步附带补偿动作。
- `TransactionScope` —— 与 `PluginScope` 类似，但带 commit / rollback 语义。

`sleep → collect → evaluate → compile → integrate → rollback` 必须通过这两个原语表达。

### 4.4 决定性时间与 ID

- `event.id`、`event.timestamp`、随机种子由 runtime 注入。
- 插件禁止直接使用 `time.time()` / `time.monotonic()` / `uuid.uuid4()` / `random` 全局源。
- 插件应通过 `context.clock` / `context.id_gen` / `context.rng` 获取。

### 4.5 可重放性

- trace + 输入序列足以回放一个 Agent tick。
- 插件副作用必须经服务接口表达，禁止依赖全局可变状态。
- 测试基础设施提供 trace 录制与回放能力（见 §5）。

### 4.6 配置 schema 自描述

- 插件 config 用 `msgspec.Struct` 声明，支持 union、版本字段、引用其他 schema。
- core 在装载时校验配置；**无 schema 的插件不允许装载**。
- 自动生成面板字段（v0.x 后由 dashboard 插件消费）。

### 4.7 Endpoint 能力协商（v0.2 改写）

> **v0.1 → v0.2 变更**：原 Adapter `StrEnum` `AdapterCapability` 已删除；能力名改注册式 `CapabilityName`，命名空间下移到 `im.*` / `tool.*`（详 [contracts.md §4](contracts.md#4-capability-命名)）。

- Source / Operation 通过 `capabilities` 字段声明能力清单：`Caps.IM_TEXT` / `Caps.IM_IMAGE` / `Caps.TOOL_INVOKE` 等（注册式，领域插件可扩展）。
- 插件 publish envelope 时声明 `capabilities_required`，dispatcher 用作 ScopeRule 匹配键之一（`ByCapability`）。
- **插件禁止硬假设平台能力**：消费 envelope 时必须用 ScopeRule（如 `ByCapability(Caps.IM_TEXT)`）显式声明前置条件，缺失即不接收。
- 详 [contracts.md §14-§18](contracts.md)。

### 4.8 双协议分离

- **内部契约**（MutsukiBot 自有）位于 `mutsukibot/contracts/`。
- **外部兼容协议**（OneBot v11/v12、MCP、ChatCompletion 等）只能出现在 adapters / 桥接插件中。
- 不得渗透 `core` / `contracts`。

### 4.9 同步点显式化

- 禁止隐式阻塞：`time.sleep`、阻塞 IO、CPU 密集裸跑。
- 必须走 runtime scheduler 提供的 awaitable / 工作池。
- Lint 规则（v0.1 起）将检查上述模式。

### 4.10 结构化错误

- 错误是一等数据对象（详见 [contracts.md](contracts.md#error)）。
- 禁止用裸字符串异常表达控制流。
- fallback 必须显式记录原因，**不允许吞异常返合理默认值**。

### 4.11 Ref 句柄生命周期

- 任何 `Handle[T]` 必须 `attach_to` 一个 `PluginScope` 或 `TransactionScope`；未绑定即报 `Error(code="handle.leak")`。
- scope 关闭时自动调用 `release` 清理；finalizer 由创建者注册（核心不假设清理逻辑）。
- 短期使用走 `with handle.borrow() as obj:`，with 块退出自动释放，避免误延长生命周期。
- 携带 `RefPayload` 字段的契约不可跨进程 / 跨隔离域；装载时静态校验 + 序列化时拒绝。详见 [contracts.md §11](contracts.md#11-通用-by-ref-payload-协议)。
- `Annotated[Handle[T], RefArg(...)]` 由 `Dependent` 类型化解析；payload 与 `ResourceHost` 两条来源都必须校验 `RefDescriptor.kind`，失败返回结构化 `ref.not_found` / `ref.kind_mismatch`。
- Lint 规则（v0.1 起）将检查直接构造 `Handle` 而不调用 `attach_to` 的模式。

### 4.12 Trace / Audit 强制（含因果链）

- 每个命令调用、工具调用、Agent tick、生命周期切换都产生结构化 trace 记录。
- 记录含 `trace_id` / `span_id` / `parent_span_id`，可串联跨插件因果链。
- `dispatch.invoke` / `dispatch.invoke_in_agent` / `ResourceHost.acquire_for` / `ResourceHost.release_for` / `ResourceHost.get_handle_for` 必须通过 `ctx.bus` 发出 span，且嵌套调用临时切换当前 span 保持父子链。
- observability 插件订阅消费。

### 4.13 Dispatcher 与跨 plugin 调用（v0.2 引入）

- 命令、跨 plugin RPC、外部 invoke 全部统一为 **Operation** 概念（详 [contracts.md §14](contracts.md)）。
- `@command` 装饰器降为 sugar：PluginMeta 在装载阶段自动包装为 `OperationDescriptor` 并通过 dispatcher 注册。
- 跨 plugin 调用路径：`await ctx.dispatch.invoke(op_id, payload)` —— **inline await 实现**，不入异步队列（保 [architecture.md §5](architecture.md) 中 sub-ms 链路的预埋承诺）。
- 事件推送：`await ctx.dispatch.publish(envelope)`，envelope.source.source_id 必须在已注册 Source 集内。
- 注册的 Operation/Source 自动 attach 到调用方 PluginScope，plugin 卸载时 dispatcher 自动反注册（无需手写清理代码）。
- Lint 规则（v0.2 起）将检查 plugin 字段直接持 raw socket / SDK client / 连接对象等违反 hard rule #14 的模式。
- 详 [contracts.md §18 Dispatcher 协议](contracts.md)。

## 5. 测试基础设施

core 必须内置以下测试支持，作为**一等公民**：

- **In-memory transport reference plugin** —— v0.2 起替代原 in-memory adapter；reference plugin 通过 dispatcher 注册 IM Source + Operation，无需真实平台即可驱动 Agent。
- **可控时钟** —— 替换 `context.clock`，支持手动推进。
- **内存事件总线** —— 同步分发，便于断言。
- **Trace 录制 / 回放** —— 支持回归测试；尊重契约的 `Replayability` 声明，从不假装能回放 `none`。
- **Trace JSONL 闭环** —— [`JsonlTraceWriter`](../mutsukibot/observability/trace.py) /
  [`JsonlTraceReader`](../mutsukibot/observability/trace.py) 负责写读同构，
  [`replay_trace_spans`](../mutsukibot/testing/trace_replay.py) 负责在测试中校验
  重复 span、父链、时间区间与可确定排序；单 bus 文件默认允许外部 parent，闭环契约
  测试显式启用完整父链校验。
- **Contract test kit** —— 一份契约测试可套用任意实现（用于 Yume / mind-sim 多实现并存场景）。
- **Handle leak detector** —— 测试结束时自动枚举未释放 `Handle`，存在即测试失败；contract test kit 强制启用，不可关闭。
- **Operation/Source 反注册检测**（v0.2 新增）—— plugin 卸载后 contract test kit 自动断言 dispatcher 中无残留 Operation/Source 注册项。leak 即测试失败。
- **Stub Handle 工厂** —— [`mutsukibot.core.handle.make_stub_handle(ref_id, *, kind, schema_id_target, schema_version_target, target, attributes)`](../mutsukibot/core/handle.py) 用于在没有真实后端（如 GPU）时生成可观测的假引用，便于上层插件单测。
- **ResourceHost 策略测试** —— host 暴露 `ResourceRecord` 给 eviction / keepalive policy；策略只能看通用 ref 元数据，不得引入领域字段解释。

测试规则：

- 单测不依赖真实 LLM、真实本地模型、网络访问、付费 API、长时间训练。
- 任何新插件必须附带「最小冒烟用例」。
- 真实模型 / GPU / 网络 / 昂贵测试单独标记或文档化为 smoke / evaluation。
- **不得以部分检查宣称成功**。

## 6. Lint 与类型

- `ruff target-version = "py313"`，`line-length = 100`。
- 启用规则集：`E F I N UP B C4 SIM RUF ASYNC ANN PTH PT`。
- 测试文件豁免：`ANN`、`PT011`。
- `pyright pythonVersion = "3.13"`，`typeCheckingMode = "standard"`。
- `pyrefly`：`project-includes = ["mutsukibot", "tests"]`、`python-version = "3.13"`。pyrefly 在元组变性、可调用类型协变上比 pyright 更严格，作为类型检查的第二把锁。
- `include = ["mutsukibot", "tests"]`。

## 7. Git 与 PR 工作流

- 默认分支：`main`。
- 任何 PR 改变契约 / 插件协议 / 生命周期阶段 / 服务接口 → 同 PR 内更新 `plans/`。
- 每个 v0.x 完成时产出 `plans/version-reports/v0.x.md`。
- 提交信息使用祈使句、英文或中文均可，但 PR 描述必须中文。
