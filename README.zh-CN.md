# Mutsuki

[English](README.md)

> **AI 实现声明：** 本项目从始至终完全由 AI 实现，不包含任何人类编写的成分。

Mutsuki 是一个具备**时空可组合性**与**可回溯执行能力**的领域中立元框架。它定义能力系统
如何被装配、演进、执行和审查，同时不把业务语义绑定到某个应用、语言或部署拓扑。

Runtime Core 只提供少量领域中立的执行法则。Host、插件、领域 Kit 和产品在这些法则之上建立
自己的框架，同时保留明确的职责边界。

## 概念模型

```text
能力空间               运行时间                  执行因果
什么可以参与组合     x 哪一个运行世界生效     -> 发生了什么、为何发生、源自哪些事实
```

### Task-System 架构

Mutsuki 借鉴了 ECS 等架构中数据与系统分离的思想，但它不是 Entity/Component 存储。**Task**
是明确的工作事实，**Runner** 提供行为，**Executor** 提供物理执行位置。因此同一个能力可以在
进程内、ABI 边界、另一种语言或远程适配器后运行，而不改变业务契约；所有路径仍使用统一的
batch-first 执行模型。

### 面向能力的空间组合

应用组合的是能力图，而不是硬编码的插件列表。能力身份独立于 scope、应用投影、实现
generation 和部署适配器。Scope 拥有服务依赖和可逆生命周期 effect；缺失的必需能力在组合
阶段失败，当前应用不支持的可选贡献则不进入 active projection。

### 基于 Generation 的时间组合

Mutsuki 通过准备新的 generation 演进运行系统，而不是原地修改当前世界。Registry 激活后
冻结，状态和资源携带版本，执行 attempt 受到授权它的 generation 约束。Reload 先构造并验证
候选世界，再切换权威状态并 drain 旧世界；过期工作无法静默提交到新 generation。

### 受控副作用与 Provenance

普通计算只描述结果，不直接改写权威状态或隐藏外部副作用。状态变化、事件、派生工作和 effect
request 都以明确事实返回，并经过受控 commit 边界。Task 状态、资源 lineage、有序事件和 trace
共同说明哪个 generation 接受了工作、哪个 attempt 执行了它，以及它产生了什么。

默认 Core 不是永久 Event Store。运行历史可以是有界、进程内的；持久审计、portable task、
checkpoint 和恢复属于可选 Host/provider 能力。外部副作用仍明确暴露真实的幂等与补偿边界，
而不会承诺并不存在的 exactly-once 执行。

## 架构边界

本仓库是一条统一兼容性基线，不是一个 package 或全局 feature 矩阵：

| 路径 | 职责 |
| --- | --- |
| `crates/mutsuki-runtime-*` | 领域中立 contract、wire、Task runtime、Host helper 和 SDK |
| `crates/link/` | Link 协议、transport、发现和 runtime adapter |
| `hosts/` | Service、CLI、桌面、Web 和可选分布式生命周期容器 |
| `kits/` | AgentKit 与 Python Runner Kit |
| `plugins/std/` | 领域中立的资源、effect、workflow、配置和观察能力 |
| `plugins/bot/` | Bot 协议、SDK、路由、平台 Adapter 和 Host integration |
| `products/bot/` | 第一方 Bot 配置、装配和产品验收 |

Core 拥有领域中立的执行事实。Host 拥有物理执行、生命周期、监督和配置。插件拥有领域能力。
产品选择 package，但不吸收 owner 的实现。外部产品只依赖需要的 package，并固定到同一个
release revision。

## Bot 参考产品

仓库内部携带了一套真实 Bot 框架和一个可运行的第一方产品，使 Mutsuki 可以被端到端审查，而
不只通过互相隔离的 runtime 示例证明自身。`plugins/bot` 拥有可复用 Bot 能力；`products/bot`
只选择配置和 owner catalog、装配 ServiceRuntime、启动产品并承载跨 package 验收。Bot 行为
不会进入 Runtime Core。

```bash
cargo run --locked -p mutsuki-bot
```

首次交互启动会在可执行文件旁创建 `.mutsuki-bot/` 实例并询问 Console 口令。非交互启动通过
`MUTSUKI_SECRET_MUTSUKI_WEB_CONSOLE_TOKEN` 提供该口令。配置与验收边界见
[Bot 产品说明](products/bot/README.md)。

创建外部 Bot 产品时，先安装 Cargo 子命令，再按提示选择 package 名、目录和固定 revision；
官方 Mutsuki 仓库由脚手架内建，无需输入：

```bash
cargo install --locked --path products/bot/crates/mutsuki-create-bot
cargo create-bot
```

脚本或 CI 可直接执行
`cargo create-bot my-bot --output <目录> --revision <40位commit>`，全程不进入交互。

## 开发

Rust package 共享根 `Cargo.toml` 和 `Cargo.lock`：

```bash
python3 skills/monorepo-maintenance/scripts/check_workspace.py
cargo metadata --locked --format-version 1
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
bash scripts/check-distributed-boundary.sh
```

Python Runner Kit 保留自己的环境：

```bash
uv run --directory kits/python-runner ruff check src tests
uv run --directory kits/python-runner pyright src tests
uv run --directory kits/python-runner pytest
```

根性能 smoke 命令是 `cargo bench-smoke`；owner baseline 与验收规则见
[性能模型](performance/README.md)。

## 参考与致谢

Mutsuki 的**时空可组合性**思想直接受到
[Cordis](https://github.com/cordiverse/cordis) 及其可逆插件模型的启发。Cordis 为这一概念
提供了起点：代码可以被同时加载且其 effect 可以被回收，构成时间可组合性；依赖关系能够被
有效声明和隔离，构成空间可组合性。

Mutsuki 在此基础上独立探索领域中立的 Task runtime、generation 执行、资源事实与 provenance；
Mutsuki 不复用 Cordis 的 API 或实现，也不声明与 Cordis 的 runtime 兼容性。

## 延伸阅读

- [单仓架构](docs/architecture/monorepo.md)
- [运行时架构](plans/architecture.md)
- [运行时契约](plans/contracts.md)
- [插件组合](docs/architecture/plugin-capability-composition.md)
- [Release Train](docs/release-train.md)
- [第一方 Bot 决策](docs/decisions/0002-first-party-bot-product.md)

## License

[MIT](LICENSE)
