# 分布式零侵入边界

本文件冻结 MutsukiCore、通用 contracts、普通 Host 与外层 Distributed Host 的依赖方向。

一句话归属规则：**单机正确性必需的能力进入 Core；单机与分布式均有明确收益的可选能力进入通用 contracts；只有多节点才有意义的能力进入 `MutsukiDistributedHost`。**

## 依赖方向

```text
plugin artifact / plugin SDK
          |
          v
Mutsuki runtime contracts
          |
          v
CoreRuntime <- ordinary HostRuntime
                    ^
                    |
          MutsukiDistributedHost adapter
                    ^
                    |
       cluster control/data/link services
```

- contracts 只能定义领域中立、可序列化、单机可解释的运行协议。
- Core 只依赖 contracts，并拥有 TaskPool、TaskLease、RunnerRegistry、ResultRouter、资源描述符、状态与事件事实。
- 普通 Host 依赖 Core/contracts，提供本机 Runtime、插件装配、执行器监督和本地控制面。
- Distributed Host 位于普通 Host 之外，只能作为普通 Host Runtime 的客户端或适配器；Core、contracts、插件 SDK 和普通 Host 不反向依赖它。
- 节点连接、发现、认证会话和 transport 可由 `MutsukiLink` 提供，但 Core、插件 SDK、ServiceHost 和业务插件不直接依赖或感知 Link。

## Distributed Host 可使用的普通 Host 表面

Distributed Host 连接某个本机 Runtime 时，只能使用普通 Host 已有的通用操作：

1. 用既有 `TaskBatch` / `TaskHandle` 提交本地任务。
2. 用既有 `TaskHandle` 取消任务。
3. 读取普通 task snapshot / outcome / health。
4. 订阅普通 runtime event / trace。
5. 通过 Host-owned resource gateway 将资源本地化后再提交任务。

普通 Host API 不增加 distributed/local 双分支。Distributed Host 完全关闭时，上述 API、插件加载、Runner 调用和本机执行行为保持不变。

## Worker 执行转换

- 远程调度描述进入 Worker 后，适配器必须先创建一个普通本地 `Task`，再通过同一 Host Runtime 提交；插件收到的 `RunnerContext`、`WorkBatch` 和资源描述符与本地任务一致。
- 远程输入、模型、checkpoint 或其他资源必须先由 Host/Distributed Host 数据面本地化为普通 `ResourceRef`。插件不下载资源，不读取节点位置，不处理网络重试或副本策略。
- 故障接管必须在另一个节点创建新的本地 Attempt，并由外层使用 fencing 拒绝旧结果。不得向插件增加 `on_failover`、`renew_lease` 或同类回调。
- 同一插件制品及其 ABI 必须能原样用于普通本地 Host 和 Worker 内的普通 Host；Worker 不拥有第二套插件 ABI。

## 旧插件默认行为

旧插件不声明任何后续可选 portable/checkpoint/restart contract 时仍然正常加载和本地执行。外层 Distributed Host 在以下任一条件成立时将任务判定为 `LocalOnly`，而不是让插件加载失败：

- payload 或资源不能持久化、内容寻址或在目标 Host 重建；
- runner 持有本地进程句柄、socket、SDK client、数据库连接或其他不可迁移状态；
- 外部副作用没有显式幂等、补偿或验证策略；
- 插件没有声明远程执行所需的通用可选契约。

该判定属于 Distributed Host 调度策略，不进入插件执行分支，也不改变旧插件 manifest 或 ABI。

## Core 与插件 SDK 禁止项

以下多节点专用事实禁止出现在 Core、通用 contracts 和插件 SDK 的公共类型、trait、feature 或依赖中：

- 节点/集群身份与位置，例如 `NodeId`、`ClusterId`、远程地址和集群资源位置；
- Leader/Follower、quorum、term、共识日志和全局复制状态机；
- 全局 assignment/lease/grant、节点 membership、故障接管和副本策略；
- peer discovery、跨机 transport、认证会话和网络重连；
- 节点 trust、attestation、执行收据或可信账本；
- `cfg(feature = "distributed")` 或任何让插件维护两套执行实现的等价开关。

本仓库允许 Host 内部已有的本地 ABI/JSONL bytes transport helper；它不是集群 transport，不能携带节点、租约、共识或调度语义。

## 自动门禁

`scripts/check-distributed-boundary.sh` 检查：

- workspace manifest 不引入 cluster、distributed、consensus、raft、quorum、跨机 transport、远程资源或 trust/attestation 专用依赖；
- contracts、Core 与 SDK 源码不出现禁止的多节点专用公共类型；
- 插件源码不出现 `distributed` feature 分支。

Host 测试还必须证明：同一 artifact identity 和同一 Runner 实现可以分别装入普通本地 Host 与 Worker 适配器内部的普通 Host，并通过完全相同的 `Task` / `Runner` 路径执行。
