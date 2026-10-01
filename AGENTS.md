# Mutsuki Monorepo 工作规范

Mutsuki 是单一 Rust workspace，同时维护 Runtime/Core、Hosts、Kits、Plugins、Products 和 Link。
目录合并不等于职责合并；每个 package 通过公开 API、协议和 path dependency 交互。

## 开始工作

1. 阅读相关 issue、`plans/{roadmap,architecture,engineering,contracts}.md` 和
   `docs/architecture/monorepo.md`。
2. 阅读最近的 scoped `AGENTS.md`，再按改动边界读取 `skills/` 下对应分组的技能。
3. 检查实现、消费者、配置/feature、文档和验证路径；Issue 只是线索，当前公开 API 才是事实源。
4. 开始和结束都运行 `git status --short`，保留已有修改；默认在当前分支工作，不创建临时工作树或 PR。

## 架构分组与技能路由

| 分组 | 目录 | 负责内容 | 首选技能 |
| --- | --- | --- | --- |
| 协议与契约 | `crates/mutsuki-runtime-contracts`、`crates/mutsuki-runtime-wire`、`plugins/*/protocols` | DTO、wire、协议 ID、错误码、跨语言镜像 | `skills/protocol/SKILL.md` |
| Runtime 与资源 | `crates/mutsuki-runtime-core`、`crates/mutsuki-runtime-host`、`crates/mutsuki-runtime-benchmarks` | TaskPool、Runner、资源、状态、effect、LoadPlan、reload | `skills/runtime/runtime-kernel/SKILL.md`、`skills/runtime/resource-state-effects/SKILL.md`、`skills/runtime/load-plan-reload/SKILL.md` |
| 组合与治理 | `crates/mutsuki-plugin-api`、`crates/mutsuki-plugin-host`、workspace 迁移 | PluginScope、Host service、生命周期、依赖和发布基线 | `skills/composition/plugin-capability-composition/SKILL.md`、`skills/governance/monorepo-maintenance/SKILL.md` |
| SDK 与执行后端 | `crates/mutsuki-runtime-sdk*`、`kits/*` | SDK、宏、Runner backend、ABI/Link 适配 | `skills/sdk/sdk-runner-host/SKILL.md` |
| Link | `crates/link` | Link 协议、transport、发现、配对、重连 | `crates/link/skills/` |
| Hosts | `hosts/*` | Service、CLI、Tauri、Web、Distributed 的运行环境和桥接 | 最近 Host 的 `skills/` |
| 领域插件 | `plugins/std`、`plugins/bot` | 标准能力、Bot 协议/SDK/Adapter/服务 | 最近 plugin 的 `skills/` |
| 产品 | `products/bot` | 配置、catalog、装配和跨 package 验收 | 最近 product 的 `skills/` |

技能只描述会改变实现决策的仓库事实；通用工作流不重复写入每个技能。技能目录必须包含带
`name` 和 `description` 的 `SKILL.md`，详细 schema 或长流程放在同目录 `references/`。

命名与职责审计读取 `skills/governance/naming-guard/SKILL.md`。

新增、移动或重命名 package、公共契约、协议 ID、验证门禁或组件命名时，同次变更更新对应
scoped `AGENTS.md` 和技能；不得把文档同步拆到后续提交。

## 不变量

- 根 `Cargo.toml`/`Cargo.lock` 是唯一 workspace 基线；内部依赖使用 root path，禁止内部 Git pin、嵌套 workspace、仓库外 `path` 和本地 `[patch]`。
- Core 保持领域中立；不得依赖具体 Host、Link transport、Agent、Bot、标准插件或产品。Host 负责装配和外部生命周期，插件不得复制 Core 或 Host。
- TaskPool、batch-first `run_batch`、`TaskHandle`、`ResourceRef`、LoadPlan、registry freeze、generation、structured failure 和 actor ownership 不得被直调绕过。
- 跨边界只传公开契约和 descriptor；Python/TypeScript/template 镜像公开 wire，不复制第二套 Core、生产 fallback 或 shim。
- Secret、账号、本地配置和真实凭据不得进入 manifest、fixture、日志、模板或版本控制。
- 资源 provider 的结果和 invalidation 由 Host actor 应用；offloaded/native async 工作在 admission 后执行，reload 和 shutdown 必须保留执行中的调用直到 actor 应用结果。

- Link 不依赖具体 Host 或业务插件；Agent/Bot/Std 仅显式 integration package 可绑定 Host。
- 不用巨型 facade crate 或根 feature 矩阵替代 package 边界；共享类型放在真实 owner。
- 测试断言行为和协议，不硬匹配日志；无功能变化不增加低价值测试。
- 提交标题使用中文短句。依赖、产品装配和发布变更须在无兄弟仓库的独立 clone 验证。

## 验证

按受影响 package 运行最小验证，然后在需要时运行完整门禁：

```text
python3 skills/governance/monorepo-maintenance/scripts/check_workspace.py
cargo metadata --locked --format-version 1
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
bash scripts/check-distributed-boundary.sh
cargo bench-smoke
```

Python、前端、集成、性能和产品验证按 scoped `AGENTS.md`/技能执行。最终说明实际命令、结果、
性能产物、release revision 及未执行的 hosted/真实账号 smoke；部分检查不得宣称全量通过。
