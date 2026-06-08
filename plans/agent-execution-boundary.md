# Agent 执行边界

当前 Rust-first 架构中，Agent 是固定运行主体，Backend / Host 是可替换行为宿主。
本文件锁定二者边界，避免把具体产品 runner 写进 core。

## 1. AgentRuntime 负责

- Agent identity 与 lifecycle phase。
- accepts 路由边界。
- inbox 与 tick。
- Source registry 与 Operation registry metadata。
- owner 候选过滤和确定性排序。
- trace 与 resource lease 的 runtime 事实。

## 2. Backend / Host 负责

- `on_awake`、`on_input`、`next_step`、`on_stop` 的具体策略。
- Source / Operation snapshot 暴露。
- Operation handler 的真实执行。
- 外部协议、工具、LLM、文件系统、浏览器、模型、领域状态等业务语义。
- 真实资源对象与 finalizer。

## 3. 不可跨越的边界

- Backend 不得修改 `agent_id`、participation、accepts 或 lifecycle phase。
- Runtime 不得保存 backend callable。
- Runtime 不得 import host / plugin / sidecar 实现。
- Backend 调用能力必须通过自身 Operation table 或 runtime 的 backend key 协议；
  不能让 runtime 直接调用兄弟能力实现。
- 外部协议和产品语义不得进入 Rust core。

## 4. 输入归属

- `primary_candidate` 才能成为 owner。
- `observer` 可用于旁路观察，但不能成为 owner。
- `explicit_helper` 不自动接收外部输入，只能被显式调用。
- `select_accepting` 只排序已满足 `awake + accepts + primary_candidate` 的候选。

## 5. 验收

- 无 Python host 时，native Rust host 能跑通 Agent start / publish / tick / invoke / stop。
- Agent 启动失败时不提交 awake，也不提交可路由 Source / Operation 事实。
- 未注册 source 被拒绝。
- Operation 调用走 backend key。
- Trace 能证明 input -> strategy 的父子关系。
