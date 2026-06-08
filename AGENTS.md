# MutsukiBot / NanoBot 项目宪法

MutsukiBot 是一个全新的 **Agent runtime kernel**，融合 Koishi、NoneBot、AstrBot 的优势，但不把 Bot / IM / command 语义放进核心。当前代码已将 IM 与文本命令路径外抽为 reference extension。

本文件同时是 NanoBot 仓库的 Agent 工作规范：所有协作者先遵守项目宪法与 Hard Rules，再按下方开发、提交、验证规程工作。

## 一句话定位

为 Yume / mind-sim 与 Lilia 式工程 Agent 提供运行核心，并通过 extension / 插件组合实现传统 Bot 框架能力。Yume / mind-sim 自身的实现路径也将被解构为本框架之上的零散插件。

## 阅读顺序

任何变更前，按以下顺序阅读相关文档：

1. [plans/roadmap.md](plans/roadmap.md) —— 当前版本目标、门控、范围。
2. [plans/architecture.md](plans/architecture.md) —— 方向、Agent 一等公民、分层、与 Yume / mind-sim 的关系。
3. [plans/engineering.md](plans/engineering.md) —— 技术栈、目录结构、插件模型、横切公约。
4. [plans/contracts.md](plans/contracts.md) —— 内部协议草案与核心契约。
5. 既有契约、实现、测试。

任何变更若没有契约位置或设计文档归属，**先设计或更新契约**，禁止直接把新机制塞进实现文件。

## 不可违反的最高规则（Hard Rules）

以下规则贯穿全框架，违反即拒绝合入。详细解释见对应文档章节。

1. **Agent 是一等运行时实体** —— 拥有身份、Context、生命周期、独立调度循环；Agent ≠ 会话 ≠ LLM 调用。
2. **核心不内置业务概念** —— LLM 调用、记忆、情感、睡眠、消息平台都必须是插件，不在 `core` 中实现。
3. **插件之间禁止直接 import 实现模块** —— 只能通过契约 + 服务 + Operation 通信。
4. **无副作用热重载** —— 卸载必须回收所有副作用；未通过 `PluginScope` 注册的副作用即视为违规。
5. **Operation 即工具** —— 同一个 Operation 声明可同时服务「人类可触发入口」与「Agent / LLM 可调用工具」manifest，禁止维护两份；文本 command 只是 reference extension 入口。
6. **无 schema 的插件不允许装载** —— 必须用 `msgspec.Struct` 声明 config schema。
7. **未申报 capability 即调用视为违规** —— Capability 必须在 manifest 中显式列出。
8. **结构化错误，不允许吞异常返默认值** —— fallback 必须显式记录原因。
9. **决定性时间与 ID 由 runtime 注入** —— 插件禁止直接用 `time.time()` / `uuid.uuid4()` / `random` 全局源。
10. **同步点显式化** —— 禁止隐式阻塞，必须走 runtime scheduler。
11. **双协议分离** —— 外部协议（OneBot / MCP / ChatCompletion 等）只能出现在 reference plugin（v0.2 起取代旧 adapters）中，不得渗透 `core` / `contracts`。
12. **Borrow with Discipline** —— 借鉴 Koishi / NoneBot / AstrBot 的心智，**不照搬代码或 API 形态**；每个机制必须能解释自己对「Agent 一等公民、解耦、可扩展」中至少一项的贡献。
13. **未声明 accepts 即拒绝路由**（v0.2 引入）—— Agent 必须显式声明 `accepts: tuple[ScopeRule, ...]`；空 tuple 等价于不接收任何 envelope（命令路径仍可用）。理由：路由层等价于 hard rule #8 的「显式 fallback 而非默认隐式」，避免 Agent 不自知地处理无关数据。详 [contracts.md §17](plans/contracts.md)。
14. **I/O 资源外置**（v0.2 引入）—— 所有 Plugin（含注册 Operation/Source 的）禁止字段直接持 raw socket / SDK client / 连接对象；必须通过 `Handle[T]` attach 到 `PluginScope`，由 finalizer 释放。理由：让「重载 plugin 逻辑」与「重连资源」在生命周期上解耦；为 v0.3+ 的 ResourceHost 跨 plugin reload 共享留出无迁移成本的接口。详 [engineering.md §4.13](plans/engineering.md)。

## Agent 工作规程

- 先读相关模块、数据契约和现有测试，再动手改代码；不要凭文件名猜边界。
- 任务复杂时先拆成明确子任务；跨 Python / Rust、core / plugin、contracts / implementation 边界的改动，优先确认契约位置和测试入口。
- 不做打补丁式修复；遇到问题先定位根因，再在正确层级修正。
- 优先沿用现有结构、命名、错误码和测试风格，不顺手做无关重构。
- 不覆盖用户或其他 Agent 的已有改动；工作前后用 `git status --short` 或定向 diff 确认范围。
- 需要长期记录的背景、取舍和未决问题写进 `plans/` 或相关设计文档；代码注释只保留能降低阅读成本的必要说明。

## 代码与契约

- **代码即事实，plans 是契约 + 决策**。公共契约、插件协议、生命周期阶段、服务接口发生变化时，**同 PR 内**更新 `plans/`。
- 跨插件能力先改 contracts / plans，再同步 core、plugin、tests；禁止把新机制直接塞进实现文件。
- 插件之间禁止直接 import 兄弟插件实现模块；跨插件调用必须走契约、服务、Dispatcher Operation 或 Source / Envelope 路由。
- `core` 保持领域中立，不出现 Yume / mind-sim / Lilia 产品语义，也不内置 LLM、记忆、情感、睡眠、IM 平台。
- `mutsukibot_ext` 与 reference plugin 可以承载外部协议翻译，但不得让 OneBot / MCP / ChatCompletion 等外部 wire shape 渗入 `core` / `contracts`。
- 插件 config 必须有 `msgspec.Struct` schema；capability、operation、source、accepts、consumes 必须显式声明。
- fallback 必须返回结构化错误并记录原因；禁止吞异常返回默认值。
- 决定性时间、ID、随机源由 runtime 注入；插件禁止直接使用全局时间、UUID 或 random 源。

## Rust / Python 边界

- Rust 2024 crate 只承载 runtime mechanics 与跨边界纯协议：Agent lifecycle、路由、Operation metadata snapshot、trace、资源租约治理。
- Rust crate 不依赖 Python，不保存 Python callable、`Handle[T]` 实体、socket / SDK client 或领域对象。
- Python 侧持有插件装载、动态 Operation handler、真实 `Handle[T]` 与领域能力；跨边界只传可序列化 snapshot、handler key、descriptor、lease token。
- 旧 generation key、backend 失败、资源租约不匹配必须 fail-loud 为结构化错误，不能自动 fallback 到看似可用的新 handler。

## Git 提交

- 提交标题用中文短句概括结果。
- 提交正文按列表简短写具体改动；无必要不写正文。
- 提交前按改动范围选择是否检查 diff；涉及多人协作、合并冲突、跨模块改动、契约变更、生命周期变更、Rust / Python 边界变更时，必须确认 diff 只包含本次改动。
- 提交前按任务复杂度做代码自检；涉及逻辑调整、重构或公共模块时，检查是否存在可删除的冗余逻辑、重复分支、无效辅助函数或代码复述型注释。

## 验证

- **不得以部分检查宣称成功**。最终说明必须报告精确执行的验证命令与结果。
- 文档、注释、配置说明等低风险改动可不跑测试；若未运行测试、构建或验证，最终说明里写清楚原因。
- Python 逻辑改动优先选择最小必要验证：
  - `uv run pytest tests -q`
  - `uv run ruff check mutsukibot tests`
  - `uv run pyright mutsukibot tests`
  - `uv run pyrefly check`
- Rust runtime / contracts 改动补跑 `cargo test`。
- 涉及公共契约、Dispatcher、ResourceHost、trace、Agent lifecycle、Rust / Python backend、热重载、capability guard 的改动，必须补充定向测试或说明现有测试覆盖点。
- 每个 v0.x 完成时产出 `plans/version-reports/v0.x.md`：方向、完成项、基线、运行检查、效果检查、下版门槛。

## 技术栈

- Python 3.13 + uv + asyncio。
- msgspec 用于契约对象、序列化与插件 config schema。
- pytest + pytest-asyncio 用于测试。
- ruff + pyright + pyrefly 用于 lint 与双类型检查。
- Rust 2024 + Cargo workspace 用于 native runtime mechanics / contracts。

详见 [plans/engineering.md](plans/engineering.md)。
