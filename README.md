# MutsukiBot

> 为 Yume / mind-sim 提供运行核心，并通过插件组合实现传统 Bot 框架能力的 Agent 中心框架。

**当前边界：Agent 事件行动核**

MutsukiBot Core 接收外部后端或协议桥转换后的事件，驱动 Agent 决策，并通过 Operation 表达 Agent 可采取的动作。Core 不内置应用后端 / CRUD endpoint / tool event 语义；真实业务状态由外部后端或领域插件持有。运行 `uv run python -m mutsukibot.plugins.echo.smoke` 验证最小闭环。

## 阅读入口

- [AGENTS.md](AGENTS.md) —— 项目宪法 + 索引 + 不可违反的最高规则
- [plans/roadmap.md](plans/roadmap.md) —— 当前版本目标、范围、门控、后续方向
- [plans/architecture.md](plans/architecture.md) —— 方向、Agent 一等公民、分层、与 Yume / mind-sim 的关系
- [plans/engineering.md](plans/engineering.md) —— 技术栈、目录、插件模型、横切公约
- [plans/contracts.md](plans/contracts.md) —— 内部协议草案

任何变更前请按上述顺序阅读相关文档；若变更没有契约位置或设计文档归属，先设计或更新契约。

## License

见 [LICENSE](LICENSE)。
