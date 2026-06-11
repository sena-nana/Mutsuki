# 未实现 / 路线图

本文档记录当前 runtime **不**包含但 contracts / hard rules 已经预留位置的能力。读完文档其他章节后，如果你期待"框架应该有 X"但找不到 X 的实现，多半在这里。

权威来源：[plans/roadmap.md](../../plans/roadmap.md) · [plans/version-reports/v0.3.md](../../plans/version-reports/v0.3.md) · [plans/version-reports/v0.3.1.md](../../plans/version-reports/v0.3.1.md) · [plans/version-reports/v0.3.2.md](../../plans/version-reports/v0.3.2.md) · [plans/version-reports/v0.4.md](../../plans/version-reports/v0.4.md)。

## codec 边界（后续）

`RefPayload[T]` 契约里说"codec 看到此结构必须二选一：拒绝编码（`Errs.REF_SERIALIZE_ATTEMPT`），或者把 handle 替换为 descriptor"。当前 runtime **没有 codec 拦截层** —— 你直接用 msgspec 把含 handle 的对象序列化，行为是未定义的（可能崩、可能成功落入意外内容）。

在 codec 边界落地之前：

- 不要把 handle 塞进 Message / Event 字段
- 跨进程边界手动 strip 成 descriptor
- 测试时仅在单进程内验证

## 跨进程隔离（v0.4+）

当前 MutsukiCore 是**单进程**。`ServiceContainer` 解析的是进程内对象；`Bus` 派发只在进程内；Agent 与 plugin 实例都在同一 event loop。

跨进程相关的契约（`ServiceMode.BY_REF` vs `BY_VALUE`、`Errs.REF_CROSS_DOMAIN`）已经定义，但运行时没有真实拦截 / 路由。多 agent 多进程的部署形态由后续版本决定。

## 完整资源调度（后续）

当前 runtime 已经有 `ResourceHost.declare_capacity()` / `acquire()` / `release()` 的进程内容量计数，并会在超额时抛 `Errs.CAPABILITY_EXHAUSTED`。v0.3.1 起，`ResourceHost` 也提供托管句柄索引、`get_handle()` / `get_handle_for()`，以及可插拔 eviction / keepalive policy；v0.3.4 起支持 `ResourceHostPolicyConfig` / `ResourceRecordSelector` 参数化治理。

尚未实现的是调度队列、自动租约心跳循环、跨进程资源迁移和与 scheduler 自动联动。按策略驱逐 / keepalive 的入口与配置契约已经存在，但不会自动后台调度。

## 类型化 Handle 注入（v0.3.1 已交付）

`Dependent.RefParam` 已能接收 `Annotated[Handle[T], RefArg(...)]`，并在解析时只认领 `Handle[T]` 注解。`RefArg(source=payload)` 支持调用 payload 中的 `Handle[T]` / `RefPayload[T]`，会校验 `RefDescriptor.kind`；`RefArg(source=resource_host)` 会通过 `ResourceHost` 服务按 `ref_id` 查找句柄，写入 `resource_host.get_handle` trace span，并在缺失或 kind 不匹配时抛结构化 `ref.not_found` / `ref.kind_mismatch`。

仍未实现的是跨进程 / 跨隔离域的真实 codec 拦截；这属于本文的「跨进程隔离」项。

## 运行时同步点检测（后续）

[hard rule #10](../../AGENTS.md) 禁止隐式阻塞 —— 插件不该 `time.sleep` / `requests.get(...)` 这种同步阻塞调用。当前依赖：

1. Code review
2. ruff `select` 中的 ASYNC 规则（间接覆盖部分场景）
3. 插件契约测试与 review 中对裸阻塞 I/O 的检查

`Errs.SYNC_VIOLATION` 错误码已定义，但触发路径未实现。

## 配置文件读取管线（v0.2+）

`PluginLoader.load_into` 已经接受 `configs: Mapping[str, object]`，并在装载阶段用 `msgspec.convert(..., type=cls.Config)` 转换 / 校验插件配置；失败会以 `Errs.PLUGIN_CONFIG_INVALID` fail-loud。

尚未实现的是"从 YAML / TOML 读 → 按 plugin id 聚合 → 注入 loader"的完整配置文件管线。要加载真实配置，目前仍需要：

1. 自己 `yaml.safe_load(...)` 或读取 TOML
2. 整理成 `{plugin_id: raw_config}` mapping
3. 传给 `PluginLoader.load_into(..., configs=...)`

后续版本会引入：从 `config/default.yaml` 读取 → 按 plugin manifest 的 `config_schema_id` 分发配置 → 支持 profile / override / secrets。

## 真实平台 transport plugin（v0.2 已交付，后续扩展）

仓库内置 transport plugin 只有：

- `InMemoryEndpointPlugin`（测试 / 冒烟）
- `OneBotV11Plugin`（OneBot v11 反向 WebSocket reference plugin）

后续仍可能增加 CLI、HTTP webhook、Discord / Telegram / 微信等参考实现，但它们都应以 plugin 形态出现，而不是独立 adapter 抽象。

## LLM provider 集成（v0.5+）

MutsukiCore 核心**不内置 LLM 调用**（hard rule #2）。LLM 必须是插件，而且是桥接插件 —— 包装外部 SDK，通过 `Caps.CALL_LLM` 声明，via service 提供给业务命令。

仓库当前没有任何 LLM provider 桥接插件。

## 持久化层

`Caps.PERSIST` 已定义，但**没有内置存储后端**。要持久化 agent 状态需要自己写 plugin（包装 sqlite / Redis / 文件等）。

## Web 控制面板 / 可视化

无内置调试面板。trace 已有 `JsonlTraceWriter` / `JsonlTraceReader` 的 JSONL 读写闭环，`mutsukicore.testing.replay_trace_spans(...)` 可校验重复 span、父链、时间区间与确定性排序；v0.4 的 `mutsukicore.testing.contract_kit` 还提供 `assert_trace_tree_closed`、`assert_cross_agent_trace_chain`、`assert_dispatcher_clean`。可视化 UI 仍在后续版本。

## 国际化 (i18n)

无。所有 docstring / 错误消息当前是中文。门面类的 attr 名是英文（`Caps.READ_MESSAGE`），但实际注册的字符串是英文蛇形（`"read_message"`）。

## 性能基准

v0.2 起已有 `dispatcher.invoke` microbenchmark gate，用于防止数量级退化。完整 throughput / latency 基准、跨 Agent invoke 端到端延迟和 ResourceHost 压测仍未建立。

## Yume / mind-sim 任何具体插件

[plans/architecture.md](../../plans/architecture.md) 提到 MutsukiCore 是给 Yume / mind-sim 做运行核 —— 但 Yume 自身的实现路径将被解构为 MutsukiCore 之上的零散插件。

v0.1 仓库**不包含** Yume 任何插件。EchoPlugin 是占位 + 形态参考。Yume 插件的引入在 v0.5+。

## 你能做什么

如果你迫切需要上述某项能力：

1. 看 [plans/roadmap.md](../../plans/roadmap.md) 当前版本目标，判断是否在路线上
2. 在 plans/ 下开 issue 讨论，明确 contracts 与硬规则的影响
3. 自己写插件（不需要等核心迭代）—— 大多数能力（持久化、LLM、平台适配）都本来就该走插件路径
4. 注意：核心改动必须**先更新契约**再实现（hard rule "代码即事实，plans 是契约 + 决策"）

## 警告：不要把路线图蓝图当当前现状

阅读 `plans/contracts.md` 时你会看到后续阶段描述。**契约形态可能已预留，运行时实现未必到位** —— 写代码前应同时看对应 version report 和源码。本节只列已知的"契约已定义、运行时尚未"项目；已交付的 v0.3.1 typed `Handle` 注入、v0.3.2 trace replay 和 v0.4 contract kit 不应再视为未实现。
