# MutsukiStdPlugins 工作规范

本仓库拥有 Mutsuki 的领域中立标准协议和通用插件实现。Core 提供 runtime 机制，Host
负责装配与生命周期；本仓库实现可复用的 config、db、fs、http、observe、resource 和
workflow 能力，不实现 Agent、Bot、产品配置或平台 UI。

## 阅读与技能路由

先读 MutsukiCore 的 `AGENTS.md`、contracts 和当前仓库公开 API，再按方向读取：

- `skills/protocol-surfaces/SKILL.md`：标准协议、DTO、schema 和 manifest surface。
- `skills/resource-state-providers/SKILL.md`：memory/shared-memory、数据库和状态 provider。
- `skills/io-effect-plugins/SKILL.md`：文件、HTTP、权限和其他 effect gateway。
- `skills/workflow-observe-plugins/SKILL.md`：workflow、广播和观测插件。
- `skills/core-conformance/SKILL.md`：Core 接入、batch-first Runner 和跨仓库验收。

## Hard Rules

1. 标准插件只实现通用领域能力，不把机制下沉到 Core，也不吸收 Agent/Bot/Host 业务。
2. 协议 crate 定义纯 wire shape；插件通过 manifest 和 RunnerDescriptor 声明真实能力。
3. Runner 只走 batch-first `run_batch`，task 操作使用 `TaskHandle`；局部失败不得污染其他 entry。
4. 外部副作用必须由 effectful Runner/Gateway 执行；资源跨边界只传 `ResourceRef`/`ValueRef`。
5. 能力、backend、permission、secret 或 LoadPlan 授权缺失时结构化失败，不做生产 fallback 或 shim。
6. Secret 只由 Host 引用和注入，不进入 manifest、fixture、日志或提交配置。
7. 禁止仓库外 Cargo `path`/本地 `[patch]`；跨仓库依赖固定远端 Git `rev`，并确保独立 checkout 可解析。

## 验证

Rust 改动运行 `cargo fmt --check`、`cargo check` 和 `cargo test`。协议、provider、effect 或
LoadPlan surface 改动补充行为测试，并报告实际命令与结果。
