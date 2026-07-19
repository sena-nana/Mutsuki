# Mutsuki Concepts

本文件只作为当前 runtime 概念入口。详细设计以以下文件为事实源：

- [roadmap.md](roadmap.md)
- [architecture.md](architecture.md)
- [contracts.md](contracts.md)

当前核心概念：

- 一切待处理控制消息都是 `Task`。
- 一切执行、编排和外部操作适配单元都通过 `Runner` 推进。
- TaskPool 是统一待处理事实源。
- Pure runner 不直接提交状态或执行副作用。
- StateStore 只通过 `core.commit` task 修改。
- 外部副作用只通过 `ProtocolClass::Effect` task 和 Effectful runner 执行；`effect.*`
  只是新 manifest 的规范命名与 legacy manifest 导入提示，不参与运行时分类。
- 资源和值以 `ValueRef` / `ResourceRef` / `StateRef` descriptor 跨边界传递。
