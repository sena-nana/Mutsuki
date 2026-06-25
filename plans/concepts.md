# Mutsuki Concepts

本文件曾记录早期 agent/message/operation 概念模型。该模型已经被 GitHub issue
#1-#3 规定的 TaskPool + Plugin Runner 架构取代。

当前概念事实源：

- [roadmap.md](roadmap.md)
- [architecture.md](architecture.md)
- [contracts.md](contracts.md)

当前核心概念：

- 一切待处理内容都是 `Task`。
- 一切执行、编排和外部操作适配单元都是 `Runner`。
- TaskPool 是统一待处理事实源。
- Pure runner 不直接提交状态或执行副作用。
- StateStore 只通过 `core.commit` task 修改。
- 外部副作用只通过 `effect.*` task 和 Effectful runner 执行。
- 资源和值以 `ValueRef` / `ResourceRef` / `StateRef` descriptor 跨边界传递。
