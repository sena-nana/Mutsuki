# MutsukiAgentKit

MutsukiAgentKit 是面向 MutsukiCore-style runtime 的 Rust-native Agent 协议 / SDK / 插件集合，不是 Core 或任何产品 Host 的扩展补丁。

本仓库不含 Python Agent SDK。

## 当前成熟度

当前是 **protocol/runtime MVP**，不是完整 Agent framework。已落地的黄金路径是 Core task
上的 model result、tool call、tool output 回灌与最终 typed result；长期 memory 策略、审批
产品体验、多 Provider 发行包和持久 session 治理仍需由对应 owner/产品继续完善。

## 分层边界

```text
MutsukiCore       TaskPool / scheduler / ResultRouter
Product Host      process supervision / plugin discovery / configuration
Platform adapters product-specific ingress and egress
MutsukiAgentKit   Agent protocols + SDK + workflow plugins
```

## 承担 / 不拥有

**承担：** Agent protocols、authoring helpers、workflow plugins、LLM/tool adapters、memory/resource helpers、conformance。

**不拥有：** Core scheduler、HostRuntime、产品配置与 Secret 解析、daemon、Python runner bridge、平台 adapters、StdPlugins baseline。不声明 Core 内建 AgentLoop / TaskGroup / actor。

## 红线

1. 不复制 scheduler；async 只经 `RuntimeClient` / `TaskHandle` / `TaskAwait`
2. memory / stream 用 `ResourceRef` / `ResourceCellRef`，不写 Core StateStore 私有路径
3. 外部副作用走 effectful runner / 标准 provider；tool call 经 TaskPool
4. 不依赖产品 Host，不读取配置文件或环境变量，不解析或保存产品 Secret

## Workspace

- `crates/mutsuki-agent-protocol` · `mutsuki-agent-sdk` · `mutsuki-agent-macros`
- `crates/mutsuki-plugin-agent-*` · `mutsuki-agent-testkit`
- `crates/mutsuki-agent-bundle` · Host-neutral Agent services 与 manifest 集合
- `schemas/` · `manifests/` · `examples/` · `templates/` · `docs/`

## 协议映射（Issue #1）

| 草案 | 稳定 id |
| --- | --- |
| plan | `mutsuki.agent/run@1` |
| tool.call | `mutsuki.agent.tool/execute@1` |
| memory.query / write | `mutsuki.agent.memory/query@1` · `.../write@1` |
| llm.complete / stream | `mutsuki.agent.model/generate@1` · `.../stream@1` |

Model gateway 是 provider-neutral orchestration runner。真实 HTTP provider 经
`effect.mutsuki.agent.model/http@1` effect runner 执行；消费端显式构造 provider 并注入
运行参数与 credential。AgentKit 不读取或管理配置，credential 不进入 task、trace 或普通日志。stream 正文保存在
provider 资源存储，runtime task 只携带 `ResourceRef`。

详见 [`docs/architecture.md`](docs/architecture.md)、[`docs/protocol.md`](docs/protocol.md)。

## Crate 边界与实际调用方

| crate | 独立边界 | 当前调用方 |
| --- | --- | --- |
| `mutsuki-agent-protocol` | Agent wire DTO / schema / protocol id | SDK 与全部 Agent runner |
| `mutsuki-agent-sdk` / `mutsuki-agent-macros` | Rust typed client、builder 与编译期 metadata | plugin crates、Rust tool/provider examples |
| `mutsuki-plugin-agent-loop` | `agent/run@1` 状态机 | `mutsuki-agent-bundle` / 产品 Host |
| context / session / memory-router / prompt | 各自协议的可选 state/resource service | `mutsuki-agent-bundle` 按产品选择注册 |
| tool-router | tool metadata 到普通 target protocol 的 TaskPool 路由 | AgentLoop 与 bundle |
| model-gateway | deterministic/provider-neutral model 编排与 HTTP effect runner | AgentLoop 与 bundle |
| `mutsuki-agent-testkit` | fake model/tool 与 conformance | workspace tests、跨仓库产品验收 |
| `mutsuki-agent-bundle` | Host-neutral manifest/runner catalog | ServiceHost 或产品装配层 |

这些 plugin crate 保留独立 manifest、runner 与可选装配边界；它们不是第二套 runtime，产品
可只注册实际选择的能力。

## Performance Model v1

Issue #4 的确定性 Agent workload 使用 `mutsuki-agent-testkit` 中版本化的 fake model/tool，
固定 seed 且禁止网络。smoke 只运行 0 μs 档；reference 运行 0 μs、1 ms、10 ms，并将
simulated model/tool time 与 AgentKit orchestration、Core/Host overhead 分开报告。

```text
python scripts/run-performance-model.py --mode smoke --output artifacts/performance/issue4-smoke.json
python scripts/run-performance-model.py --mode reference --process-runs 3 --output artifacts/performance/issue4-reference.json
```

覆盖 single-turn、tool-once、chain-8、真实 batched parallel-8、session-100、三档 context、
memory route、wait/resume、cancel 与 failure/retry。详见
[`docs/performance-model-issue4.md`](docs/performance-model-issue4.md)。
