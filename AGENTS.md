# MutsukiBot / NanoBot 项目宪法

MutsukiBot 当前是一个 **Rust-first Agent runtime framework**。根目录只承载领域中立
runtime kernel、纯协议契约和 native host helper；早期 Python 框架实现已经移动到
`python/reference-mutsukibot/`，作为旧 Python 实现的参考与迁移层，不代表废弃内容。

## 一句话定位

为 Yume / mind-sim、工程 Agent 与传统 Bot 能力提供领域中立运行核心。Rust core
只负责 Agent lifecycle、routing、Operation / Source metadata、resource lease 和
trace 等运行机制；具体业务能力由 host、plugin、sidecar 或 reference layer 组合实现。

## 阅读顺序

任何变更前按以下顺序阅读：

1. [plans/roadmap.md](plans/roadmap.md) —— 当前 Rust-first 目标、门控、范围。
2. [plans/architecture.md](plans/architecture.md) —— 分层与领域中立边界。
3. [plans/engineering.md](plans/engineering.md) —— workspace、crate 边界、验证规则。
4. [plans/contracts.md](plans/contracts.md) —— Rust runtime 内部协议。
5. 既有契约、实现、测试。

没有契约位置或设计文档归属的新机制，先更新 plans / contracts，再写实现。

## Hard Rules

1. **Agent 是一等运行时实体**：Agent 拥有身份、生命周期、accepts、inbox 和运行时状态；Agent 不等于会话、LLM 调用或 handler。
2. **核心不内置业务概念**：LLM、记忆、情感、睡眠、IM、MCP、ChatCompletion、Yume、Lilia、工程工具等不得进入 Rust core。
3. **Operation 即工具**：命令、人类入口、LLM tool、跨能力调用统一表达为 Operation；Rust runtime 只持有 descriptor / snapshot / backend key。
4. **Source 必须显式注册**：`publish(envelope)` 必须校验 `source_id` 已注册；未注册 source 结构化失败为 `source.unregistered`。
5. **未声明 accepts 即拒绝路由**：Agent `accepts` 为空时不接收任何 envelope。
6. **结构化错误**：fallback 必须显式记录原因；禁止吞异常返回默认值。
7. **决定性时间与 ID 由 runtime / host 注入**：core 不直接调用全局时间、UUID 或 random 源；资源租约 token 由注入式 ID source 生成。
8. **I/O 资源外置**：Rust core 不保存 socket、SDK client、数据库连接、Python callable、真实 `Handle[T]` 或领域对象。
9. **双协议分离**：外部 wire shape 只能在 host / reference plugin / Python sidecar；不得渗入 `crates/mutsuki-runtime-core` 或 `contracts`。
10. **Borrow with Discipline**：按引用传递只通过 `RefDescriptor`、`ref_id`、`LeaseToken` 和 host-owned finalizer 表达；真实对象不跨 runtime 边界。

## 工作规程

- 先读相关 crate、契约和测试，再改代码；不要凭文件名猜边界。
- 跨 contracts / core / host / Python reference 边界的改动，先确认契约位置和测试入口。
- 不做打补丁式修复；定位根因，在正确层级修正。
- 优先沿用现有命名、错误码和测试风格。
- 不覆盖用户或其他 Agent 的已有改动；工作前后用 `git status --short` 或定向 diff 确认范围。
- 需要长期记录的背景、取舍和未决问题写进 `plans/`。

## Rust / Python Reference 边界

- Root Rust crates 不依赖 Python。
- `python/reference-mutsukibot/` 是旧 Python 框架、扩展、测试、docs 和 examples 的参考与迁移位置。
- 如果未来需要 Python sidecar，它只能通过 backend trait / 纯协议与 Rust runtime 通信。
- 旧 generation key、backend 失败、资源租约不匹配必须 fail-loud；不能 fallback 到看似可用的新 handler。

## Git 提交

- 提交标题用中文短句概括结果。
- 提交正文按列表简短写具体改动；无必要不写正文。
- 涉及契约、生命周期、backend trait、资源治理或目录重组时，提交前必须检查 diff 范围。

## 验证

- 不得以部分检查宣称成功。最终说明必须报告精确执行的验证命令与结果。
- Rust runtime / contracts / host 改动必须运行：
  - `cargo test`
- 改动 Python reference 层时，从 `python/reference-mutsukibot` 目录运行对应 Python 验证。
- 涉及公共契约、Source / Operation registry、ResourceGate、trace、Agent lifecycle、
  backend trait 的改动必须补充定向测试或说明现有测试覆盖点。

## 技术栈

- Rust 2024 + Cargo workspace 是根级主框架。
- serde / serde_json 用于纯协议序列化。
- thiserror 用于 runtime failure wrapper。
- Python 3.13 + uv 仅用于 `python/reference-mutsukibot/`。

详见 [plans/engineering.md](plans/engineering.md)。
