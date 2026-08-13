# Mutsuki Monorepo 工作规范

本仓库是 Mutsuki Framework 的唯一主仓。它使用单仓库、多 package 的 Rust Workspace，
同时承载 Python Runner Kit、前端 SDK/Shell、第一方 Bot 产品、跨 package 集成测试、性能模型和
统一发布资料。仓库合并不等于 package 合并；每个 package 继续拥有清晰职责和可选依赖。

## 阅读顺序

1. 当前及关联 Issue，确认 package owner、依赖和验收场景。
2. `plans/{roadmap,architecture,engineering,contracts}.md`。
3. `docs/architecture/monorepo.md` 与改动目录最近的 `AGENTS.md`。
4. 根技能及 scoped `skills/*/SKILL.md`，再检查实现、测试和发布资料。

Issue 是需求线索，不是当前 API 的事实源。存在 `.codegraph/` 时，定位代码先用 CodeGraph。

## 根技能路由

- `skills/monorepo-maintenance/SKILL.md`：目录、内部依赖、历史/Issue 迁移、Release Train、
  兼容矩阵、第一方产品和归档。
- `skills/contracts/SKILL.md`：公共 DTO、协议 ID、错误码、序列化与跨语言契约。
- `skills/runtime-kernel/SKILL.md`：TaskPool、Runner、批处理、路由、取消与 trace。
- `skills/resource-state-effects/SKILL.md`：资源 descriptor、lease、状态、事件与 effect。
- `skills/load-plan-reload/SKILL.md`：manifest、LoadPlan、registry generation 与热重载。
- `skills/plugin-capability-composition/SKILL.md`：PluginScope、scoped service、effect ownership、
  contribution projection 与 staged reload。
- `skills/sdk-runner-host/SKILL.md`：Rust SDK、宏、Runner host helper 与通用 ABI。

跨 package 或目录移动先读 monorepo-maintenance；跨协议边界同时读 contracts。

## 目录与职责

| 路径 | 职责 |
| --- | --- |
| `crates/mutsuki-runtime-*` | 领域中立 contracts、wire、Core、Host helper、Rust SDK 与基准 |
| `crates/link/` | Link 协议、传输、发现、配对和 runtime adapter |
| `hosts/cli/` | ServiceHost 控制 API 的 CLI/TUI 客户端 |
| `hosts/service/` | 常驻服务生命周期、配置/secret、插件加载、Runner 监督、控制面和 health |
| `hosts/tauri/` | 桌面生命周期、Tauri/WebView bridge、资源与前端 SDK |
| `hosts/web/` | Web Host、HTTP/WS、WebExtension、Recovery Shell 与 Web 包 |
| `hosts/distributed/` | 可选分布式 sidecar、持久化、恢复、控制和基准 |
| `kits/agent/` | Agent 协议、SDK、插件、testkit 与 bundle |
| `kits/python-runner/` | Python Runner SDK、wire mirror、transport 与 conformance |
| `plugins/bot/` | Bot 协议、SDK、路由、平台 Adapter、Host integration 与 testkit |
| `plugins/std/` | 通用协议、资源/provider、effect、workflow 与 observe 插件 |
| `products/bot/` | 第一方 Bot 产品入口、配置、运行装配与跨 package 验收 |

## Hard Rules

1. 根 `Cargo.toml` 和 `Cargo.lock` 是唯一 Rust Workspace 与版本基线；禁止新增嵌套
   Workspace、嵌套 lockfile 或仓库内 Mutsuki Git 依赖。
2. Core 不依赖具体 Host、Link transport、Agent、Bot、标准插件或产品；Link 不依赖具体
   Host 和业务插件；Agent/Bot/Std 默认不绑定 Host，显式 integration package 除外。
3. 不建立默认包含全部能力的巨型 crate，不用根 feature 矩阵替代 package 边界。
4. TaskPool、batch-first Runner、TaskHandle、ResourceRef、LoadPlan、registry freeze、
   structured failure 等运行时不变量不得因 monorepo 快捷直调而绕过。
5. package 间只通过公开 API 和仓内 path 依赖交互；共享类型进入真实 owner，禁止公共杂物包。
6. Python、TypeScript 和模板必须镜像公开契约，不复制第二套 Core 或生产 fallback/shim。
7. 第一方 `mutsuki-bot` 产品位于 `products/bot`；其他业务产品继续位于独立仓库，只依赖
   所需 package，并固定统一 release tag 或 commit。
8. 迁移必须保留 Git 历史、有效 Issue、评论和回链；源仓有独占可执行任务时禁止归档。
9. Secret、账号和本地配置不得进入模板、fixture、日志、manifest 或版本控制。
10. 修复根因并在 owner package 落地；禁止因同仓而跨层打补丁、隐式耦合或假能力。
11. 新测试断言行为和协议，不硬匹配日志/文案；无功能变化不添加低价值测试。
12. 禁止临时分支工作树修改。默认在用户当前分支工作；除非用户明确要求，不创建 PR。

## Git 与验证

- 工作前后检查 `git status --short`；保留用户已有修改。
- 提交标题用中文短句概括结果；目录迁移、公共契约和生命周期变更提交前检查 diff 范围。
- 根 Rust 门禁：
  - `python3 skills/monorepo-maintenance/scripts/check_workspace.py`
  - `cargo metadata --locked --format-version 1`
  - `cargo fmt --all -- --check`
  - `cargo check --workspace --all-targets --locked`
  - `cargo test --workspace --all-targets --locked`
  - `bash scripts/check-distributed-boundary.sh`
  - `cargo bench-smoke`
- Python、前端、集成和 owner 性能测试按 scoped AGENTS/SKILL 执行。
- 依赖、产品装配或发布改动必须在无兄弟仓库的独立 clone 验证。
- 最终说明列出实际命令、结果、性能产物、release revision 与未执行的外部 smoke；不得以
  部分检查宣称成功。
