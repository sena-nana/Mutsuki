# 未实现 / 路线图

本文档记录 v0.1 **不**包含但 contracts / hard rules 已经预留位置的能力。读完文档其他章节后，如果你期待"框架应该有 X"但找不到 X 的实现，多半在这里。

权威来源：[plans/roadmap.md](../../plans/roadmap.md) · [plans/version-reports/v0.1.md](../../plans/version-reports/v0.1.md)。

## codec 边界（v0.2 候选）

`RefPayload[T]` 契约里说"codec 看到此结构必须二选一：拒绝编码（`Errs.REF_SERIALIZE_ATTEMPT`），或者把 handle 替换为 descriptor"。v0.1 **没有 codec 拦截层** —— 你直接用 msgspec 把含 handle 的对象序列化，行为是未定义的（可能崩、可能成功落入意外内容）。

约束在 v0.2 落地。在那之前：

- 不要把 handle 塞进 Message / Event 字段
- 跨进程边界手动 strip 成 descriptor
- 测试时仅在单进程内验证

## 跨进程隔离（v0.3+）

当前 MutsukiBot 是**单进程**。`ServiceContainer` 解析的是进程内对象；`Bus` 派发只在进程内；Agent 与 plugin 实例都在同一 event loop。

跨进程相关的契约（`ServiceMode.BY_REF` vs `BY_VALUE`、`Errs.REF_CROSS_DOMAIN`）已经定义，但运行时没有真实拦截 / 路由。多 agent 多进程的部署形态由后续版本决定。

## CapabilityCounter / 资源量化（v0.3+）

`Capability(name=..., quantity={"tokens_per_min": 100000})` 这种写法当前**只是 manifest 元数据** —— 调度器不会因为 token 用超就拒绝调用。`Errs.CAPABILITY_EXHAUSTED` 错误码定义了，但触发路径未实现。

资源量纲的实际计数器、配额管理、回压路径在后续版本。

## 运行时同步点检测（v0.2 候选）

[hard rule #10](../../AGENTS.md) 禁止隐式阻塞 —— 插件不该 `time.sleep` / `requests.get(...)` 这种同步阻塞调用。当前依赖：

1. Code review
2. ruff `select` 中的 ASYNC 规则（间接覆盖部分场景）

[runtime/loop.py](../../mutsukibot/runtime/loop.py) 留了 `install_sync_point_guard(callback)` 占位钩子。完整的基于 `sys.settrace` 或 ruff 自定义规则的拦截在后续版本落地。

`Errs.SYNC_VIOLATION` 错误码已定义，但触发路径未实现。

## 配置 schema 自动校验（v0.2 候选）

当前 `PluginLoader.load_into` 接受 `configs: dict[plugin_id, msgspec.Struct]`，但没有"从 YAML / TOML 读 → 校验 schema → 注入 plugin"的自动管线。要加载真实配置，需要：

1. 自己 `yaml.safe_load(...)` + `msgspec.convert(...)`
2. 失败时手动构造错误并抛

后续版本会引入：从 `config/default.yaml` 读取 → 按 plugin manifest 的 config_schema_id 自动校验 → 装载前拒绝不通过的。

## 真实平台 adapter（v0.2 候选）

仓库内置 adapter 只有：

- `InMemoryAdapter`（测试 / 冒烟）

CLI（typer）+ 至少一个 IM 平台（OneBot v11 或 WS）adapter 在 v0.2 候选范围。Discord / Telegram / 微信等需要后续版本或第三方插件。

## LLM provider 集成（v0.5+）

MutsukiBot 核心**不内置 LLM 调用**（hard rule #2）。LLM 必须是插件，而且是桥接插件 —— 包装外部 SDK，通过 `Caps.CALL_LLM` 声明，via service 提供给业务命令。

仓库当前没有任何 LLM provider 桥接插件。

## 持久化层

`Caps.PERSIST` 已定义，但**没有内置存储后端**。要持久化 agent 状态需要自己写 plugin（包装 sqlite / Redis / 文件等）。

## Web 控制面板 / 可视化

无内置。trace 当前落 JSONL，靠 `jq` / `cat` 看；调试面板在更后续版本。

## 国际化 (i18n)

无。所有 docstring / 错误消息当前是中文。门面类的 attr 名是英文（`Caps.READ_MESSAGE`），但实际注册的字符串是英文蛇形（`"read_message"`）。

## 性能基准

无。v0.1 测试只验证功能正确，不做 throughput / latency 基准。当前规模（数十订阅、数百插件、数千 trace span/min）应当可接受，但没有数据支撑。

## Yume / mind-sim 任何具体插件

[plans/architecture.md](../../plans/architecture.md) 提到 MutsukiBot 是给 Yume / mind-sim 做运行核 —— 但 Yume 自身的实现路径将被解构为 MutsukiBot 之上的零散插件。

v0.1 仓库**不包含** Yume 任何插件。EchoPlugin 是占位 + 形态参考。Yume 插件的引入在 v0.5+。

## 你能做什么

如果你迫切需要上述某项能力：

1. 看 [plans/roadmap.md](../../plans/roadmap.md) 当前版本目标，判断是否在路线上
2. 在 plans/ 下开 issue 讨论，明确 contracts 与硬规则的影响
3. 自己写插件（不需要等核心迭代）—— 大多数能力（持久化、LLM、平台适配）都本来就该走插件路径
4. 注意：核心改动必须**先更新契约**再实现（hard rule "代码即事实，plans 是契约 + 决策"）

## 警告：不要把 v0.2 蓝图当 v0.1 现状

阅读 `plans/contracts.md` 时你会看到很多 "v0.2 才会引入" 的描述（codec 拦截、跨进程边界、配额计数等）。**契约形态已锁，运行时实现没到** —— 写代码假设它们存在会让你失望。本节列的就是已知的"契约已定义、运行时尚未"项目。
