# Mutsuki 项目宪法

Mutsuki 当前是一个 **Rust-first CoreRuntime framework**。根目录只承载领域中立
runtime kernel、纯协议契约和 native runner host helper；早期 Python 框架实现保留在
`python/reference-mutsuki/`，只作为旧实现参考与迁移材料，不代表当前主链。

## 一句话定位

为 Yume / mind-sim、工程 runner 与传统 Bot 能力提供领域中立运行核心。Rust core
只负责 TaskPool、RunnerRegistry、RunnerLoop、ResultRouter、StateStore、
ResourceManager、EventLog、TraceLog 和 load-plan 校验；具体业务能力由 host、
plugin、runner 或 sidecar 组合实现。

## 阅读顺序

任何变更前按以下顺序阅读：

1. [plans/roadmap.md](plans/roadmap.md) —— 当前 CoreRuntime 目标、门控、范围。
2. [plans/architecture.md](plans/architecture.md) —— 分层与领域中立边界。
3. [plans/engineering.md](plans/engineering.md) —— workspace、crate 边界、验证规则。
4. [plans/contracts.md](plans/contracts.md) —— Rust runtime 纯协议。
5. 既有契约、实现、测试。

没有契约位置或设计文档归属的新机制，先更新 plans / contracts，再写实现。

## Hard Rules

1. **Task 是一等运行事实**：所有待处理控制消息进入 `TaskPool`；不恢复早期实例私有队列或多队列调度形态作为核心事实源。
2. **Runner 是唯一执行单元**：插件通过 `RunnerDescriptor` 声明可处理的 task kind、schema、purity 和 generation；core 只注册、claim、调用和路由结果。
3. **核心不内置业务概念**：LLM、记忆、情感、睡眠、IM、MCP、ChatCompletion、Yume、Lilia、工程工具等不得进入 Rust core。
4. **副作用必须 task 化**：Pure runner 只能返回 `Task`、`DomainEvent`、`StateDelta`、`EffectRequest`；外部副作用必须变成 `effect.*` task，由 Effectful runner 执行。
5. **状态只能通过 Committer 提交**：StateStore/EventLog 只由 `core.commit`、`core.event.append` 等 kernel task 和 Committer runner 修改。
6. **ResourceRef/ValueRef 是 descriptor**：跨 runtime 边界只能传 ref descriptor、schema、generation、lifetime、lease 和访问方式；不得传 Python object、Rust pointer、Arc、Vec 本体、socket、SDK client、数据库连接或真实 handle。
7. **共享数据默认 readonly/sealed**：修改生成新 ref；确需原地写必须持有有效 `ExclusiveWriteLease`，lease 过期、generation mismatch、provider 崩溃必须结构化失败。
8. **LoadPlan 是 registry 权威**：core boot 只消费 resolver 生成的 `RuntimeLoadPlan/RuntimeLock`；runner、task demand、resource、effect descriptor 不得超出 load plan 授权。
9. **Registry freeze 后禁止动态注册**：运行中新增能力必须生成新的 registry generation 和 load plan。
10. **热重载不原地替换**：新插件 generation 通过 Identical/Additive/Deprecated/Removed/Breaking surface 比较进入；Deprecated 禁止新增占用，Removed 必须 zero occupancy，Breaking 必须 migration/drain/restart。
11. **决定性时间与 ID 由 runtime / host 注入**：core 不直接调用全局时间、UUID 或 random 源；资源租约 token 由注入式 ID source 生成。
12. **结构化错误**：fallback 必须显式记录原因；禁止吞异常返回默认值。

## 工作规程

- 先读相关 crate、契约和测试，再改代码；不要凭文件名猜边界。
- 跨 contracts / core / host / Python runner kit 边界的改动，先确认契约位置和测试入口。
- 不做打补丁式修复；定位根因，在正确层级修正。
- 优先沿用现有命名、错误码和测试风格。
- 不覆盖用户或其他 Agent 的已有改动；工作前后用 `git status --short` 或定向 diff 确认范围。
- 需要长期记录的背景、取舍和未决问题写进 `plans/`。

## Rust / Python 边界

- Root Rust crates 不依赖 Python。
- `python/mutsuki-runtime-python/` 是当前 Python runner kit，必须镜像新 contracts wire shape。
- `python/reference-mutsuki/` 是旧 Python 框架、扩展、测试、docs 和 examples 的参考与迁移位置。
- Python sidecar 只能通过 runner step、management cancel/dispose 和 resource broker 纯协议与 Rust runtime 通信。
- 旧 generation key、runner host 失败、资源租约不匹配必须 fail-loud；不能 fallback 到看似可用的新 handler。

## Git 提交

- 提交标题用中文短句概括结果。
- 提交正文按列表简短写具体改动；无必要不写正文。
- 涉及契约、生命周期、runner trait、资源治理或目录重组时，提交前必须检查 diff 范围。

## 验证

- 不得以部分检查宣称成功。最终说明必须报告精确执行的验证命令与结果。
- Rust runtime / contracts / host 改动必须运行：
  - `cargo fmt --check`
  - `cargo test`
- 改动 Python runner kit 时，从 `python/mutsuki-runtime-python` 目录运行：
  - `uv run ruff check src tests`
  - `uv run pyright src tests`
  - `uv run pytest`
- 涉及公共契约、RunnerRegistry、ResultRouter、ResourceManager、trace、StateStore、
  load plan 或 hot reload 的改动必须补充定向测试或说明现有测试覆盖点。

## 技术栈

- Rust 2024 + Cargo workspace 是根级主框架。
- serde / serde_json 用于纯协议序列化。
- thiserror 用于 runtime failure wrapper。
- Python 3.13 + uv 用于 `python/mutsuki-runtime-python/` 和旧 reference。

详见 [plans/engineering.md](plans/engineering.md)。
