# MutsukiBot

> 为 Yume / mind-sim 提供运行核心，并通过插件组合实现传统 Bot 框架能力的 Agent 中心框架。

**当前版本：v0.1 最小可运行骨架**

v0.1 落地了第一个可装载、可运行、可被测试的 Agent 闭环（echo 插件 + in-memory adapter + 完整 contracts/core/runtime 分层 + 45 项测试）。运行 `uv run python -m mutsukibot.plugins.echo.smoke` 验证。详见 [plans/version-reports/v0.1.md](plans/version-reports/v0.1.md)。

## 阅读入口

- [AGENTS.md](AGENTS.md) —— 项目宪法 + 索引 + 不可违反的最高规则
- [plans/roadmap.md](plans/roadmap.md) —— 当前版本目标、范围、门控、后续方向
- [plans/architecture.md](plans/architecture.md) —— 方向、Agent 一等公民、分层、与 Yume / mind-sim 的关系
- [plans/engineering.md](plans/engineering.md) —— 技术栈、目录、插件模型、横切公约
- [plans/contracts.md](plans/contracts.md) —— 内部协议草案

任何变更前请按上述顺序阅读相关文档；若变更没有契约位置或设计文档归属，先设计或更新契约。

## License

见 [LICENSE](LICENSE)。
