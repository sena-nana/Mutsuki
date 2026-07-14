# 可选任务持久化、恢复与内容寻址契约

本文冻结不依赖部署拓扑的任务录制、重放、checkpoint 和内容寻址边界。这些契约可由
单机持久队列、测试重放工具、跨进程 Host 或外层部署系统复用；CoreRuntime 默认不执行
持久化、哈希扫描、传输或恢复策略。

## 现有契约审计

- `Task` 可 serde，包含重建执行所需的 protocol、payload、refs、trace/correlation 和
  scheduling descriptor，但 `lease_id`、registry generation 与 created sequence 是本地
  runtime attempt 事实，不能原样持久化。
- `TaskHandle` 是查询/取消 descriptor，不含 payload，不能独立重建 task。
- `TaskLease` 绑定 runner、executor、registry generation 和 attempt 时限，只用于本地
  fencing，不能作为 portable execution identity。
- 因此 `PortableTask` 包装普通 `Task`，显式携带 task schema 与 input `ContentId`，并在
  录制和恢复时清除 attempt-local 字段。恢复结果仍是普通本地 `Task`。

## 独立可选能力

`PortabilityCatalog` 独立于现有 `PluginManifest` / `RunnerDescriptor` wire shape：插件或
Host 只有在显式提供 catalog 时才启用能力。catalog 中没有对应 protocol 时，能力解析为
`ExecutionMobility::LocalOnly`。旧插件不修改、不重新编译也继续沿原执行路径运行。

通用能力枚举：

- `ExecutionMobility`：LocalOnly、Portable、Restartable、Checkpointable。
- `RetrySafety`：Idempotent、Verifiable、Compensatable、Unsafe。
- `TaskAcceptanceDurability`：Volatile、Buffered、Persisted。
- `ResourcePersistence`：Ephemeral、Durable、ContentAddressed。
- `RecoveryMode`：Unavailable、RestartFromInput、RestoreCheckpoint。

这些值描述能否被某个外部 Host/工具使用，不触发 Core 后台行为，也不承诺 exactly-once
副作用。

## Portable task 与流式编解码

`PortableTask` 包含版本化 envelope schema、task schema、能力、输入内容标识、可选 portable
resource descriptor 和普通 `Task`。portable resource 通过 `task_ref_id` 映射回 task 的
逻辑 input ref。`write_json` / `read_json` 直接面向 `Write` / `Read`，
调用方可连接文件、pipe 或增量 buffer，无需 Core 保存完整持久队列。

`into_local_task` 清除 lease、registry generation 和 created sequence，Host 再按本地 load
plan 入池。portable resources 只携带 `ContentId`、kind、schema 和 persistence；外层 Host
必须先从自己的存储 materialize 为本地 `ResourceRef`，插件仍只消费既有 ResourceRef。

## Checkpoint/restart

`TaskCheckpoint` 包含：

- checkpoint 与 task schema identity；
- plugin implementation generation；
- input content digest/size/format；
- 单调 checkpoint sequence；
- portable task 与 plugin-owned opaque payload。

恢复前使用 `is_self_consistent` 和 `is_compatible_with` 检查 envelope、task schema、实现
generation 与 input identity。SDK 的 `Checkpointable` 是可选 side contract，不修改
`Runner::run_batch`；未实现者由外部策略选择 RestartFromInput 或 Unavailable，也不会收到
failover 回调。

## ContentId 与资源边界

`ContentId` 只描述 algorithm、digest、size、format，可用于本地去重、完整性校验、缓存和
快照复用。contracts 不计算 digest，也不扫描 `ResourceRef`；何时计算、保存和 materialize
由使用该可选能力的 Host/provider 决定。

portable descriptor 不含部署位置、传输入口、复制策略或一致性角色。它不是
`ResourceRef` 的替代品，也不允许插件绕过 ResourceManager/provider gateway。

## 零开销默认路径

CoreRuntime、Runner ABI、Task submit、PluginManifest 和 load plan 均未增加必填字段或后台
服务。未创建 `PortabilityCatalog` / `PortableTask` / `TaskCheckpoint` 时，不分配 portable
payload，不启动线程，不执行哈希、网络或持久化 I/O。
