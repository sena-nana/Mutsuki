# AgentKit architecture

AgentKit 是 MutsukiCore 之上的三个一级领域，不是第二套 Core 或产品 Host。

```text
Product profile / Persona compiler / UI
                 |
       AgentRuntimeProfile
                 |
Runtime -------- Adapter -------- Plugin
 session          protocol         tool/context/hook
 turn             provider         policy/command/service
 approval         stream
 budget
                 |
 Mutsuki Task / Runner / Resource / Capability / Plugin lifecycle
                 |
 HostRuntime / MutsukiLink / Database service / DistributedHost
```

## Runtime

`mutsuki-agent-runtime` 拥有 session coordinator、turn 状态、approval、budget、checkpoint
语义和 `AgentRuntimeProfile` validation。昂贵或可取消的步骤由 orchestration runner 通过
`TaskAwaitRunnerAdapter` 提交回 Core；Runtime 不拥有 scheduler、worker、ResultRouter
或通用重试器。

## Adapter

`mutsuki-agent-adapter-api` 定义统一请求、流事件、Provider instance descriptor 和错误
分类。具体协议映射由 Adapter package 实现为 Mutsuki `AsyncBatchHandler`。Provider 品牌、
端点和凭据是产品注入的实例数据，不进入 Agent 公共 contract。

## Plugin

`mutsuki-agent-plugin-api` 定义 tool、context provider、hook、policy、command 和 service
贡献。装载、依赖、权限、generation、drain-and-swap 和 service registry 都由 Mutsuki
插件生命周期兑现。LSP 是共享 service + Plugin 的参考高级实现，Host 不认识 LSP 语义。

## 状态与基础设施

- 消息、快照、流和大型工具结果使用 `ResourceRef` / `ResourceCellRef`。
- durable checkpoint 经 `mutsuki-protocol-db` service，不内置数据库。
- remote task/sub-agent 映射到 DistributedHost placement，不复制选主与 scheduler。
- client wire 运行在 MutsukiLink control stream，不建立 Agent 专属网络 server。
- ServiceHost/TauriHost 只提供公开 runner、async handler、service 和 bridge 装配入口。

## 产品边界

产品拥有 workspace/file/selection/cursor、Persona、默认 Provider、Secret、diff preview、
approval UI、session 列表和业务 command。Persona 可编译为 system instructions、prompt
fragments、allowlist 和 policy，但 AgentKit 不定义 Persona 类型，也不需要
`LiliaCodeCore`。

## 不变量

- 不绕过 TaskPool 直调模型或工具。
- 不复制 scheduler、Host lifecycle、Link transport、数据库引擎或分布式协调器。
- 不通过字符串前缀承担关键路由；使用 protocol/descriptor/typed DTO。
- production bundle 不注册 fake/mock Provider 或 fallback。
- feature flag 不改变同名 contract 的基础语义。
