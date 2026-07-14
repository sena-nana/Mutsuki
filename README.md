# MutsukiBotTemplate

配置驱动、实现中立的 Mutsuki Bot 产品装配器。`mutsuki-bot` 只加载 ServiceHost 配置、
注册 owner 提供的插件 factory catalog 并启动 Runtime；它不实现命令、回复、Agent 流程或
任何具体业务 Bot。

## Run

创建本地配置和 Secret 文件：

```powershell
Copy-Item config/template.toml config/local.toml
Copy-Item config/secret.template.toml config/local.secret.toml
cargo run -p mutsuki-bot
```

配置路径优先级为命令行参数、`MUTSUKI_CONFIG`、`config/local.toml`：

```powershell
cargo run -p mutsuki-bot -- path/to/product.toml
```

提交的 `config/template.toml` 是零插件的中立产品。零插件 Runtime 可以启动和停止，但没有
平台连接或业务行为。最终产品从已经链接或安装的 owner 插件中选择能力：

```toml
[[plugins.configured]]
id = "owner.plugin.id"

[plugins.configured.config]
# 仅由插件 owner 定义和解析
```

同一份配置同时适用于 builtin 与 ABI。Native factory 由对应依赖仓库链接进 catalog；外部
artifact 由 ServiceHost 插件目录形成库存，但不会因文件存在而自动启用。Host 在只有一种部署
时直接选择，同时存在 builtin/ABI 时默认 builtin；管理工具可把部署偏好写入 Host 状态而无需
修改业务配置。新增业务能力应在 BotPlugins、AgentKit 或独立业务仓库实现并发布，禁止把业务
Runner 复制到本模板。

外置 ABI 包使用 Core SDK 的版本化 JSONL byte transport，并按
`<dynamic_dir>/<plugin>/plugin.toml + DLL/SO/dylib` 安装。`artifact.path` 必须留在插件目录，
`artifact.sha256` 必须匹配文件；ServiceHost 在 LoadPlan 冻结前完成校验、ABI v2
`plugin.initialize` 和
Runner/ResourceProvider 注册。ABI 动态库是可信进程内代码，需要隔离时应选择 Process/Python
部署。

主配置只保存 Secret key 引用，实际值放在被 Git 忽略的 `config/local.secret.toml`，或使用
`MUTSUKI_SECRET_<KEY>` 环境变量覆盖。默认 Runtime home 是 `~/.mutsuki`，其下包含
`data`、`logs`、`plugins` 和 `run`。

## QQ Gateway smoke

QQ 只是可选平台插件和验收场景。自动测试使用 BotPlugins fake 验证配置装配、Gateway、
health、Resume 和 graceful shutdown，不包含命令或回复：

```powershell
cargo test -p mutsuki-bot --test qqbot_config_e2e
```

macOS/Linux 还会启动真实 `mutsuki-bot` 产品进程，通过 Unix socket 验证 health、控制面
shutdown、Gateway Identify/Resume、WebSocket clean close 和 socket 清理：

```powershell
cargo test -p mutsuki-bot --test unix_product_smoke
```

真实账号 smoke 只验证鉴权、Gateway 连接和 health：

```powershell
cargo test -p mutsuki-bot --test qqbot_real_smoke -- --ignored --nocapture
```

## Verification

```powershell
cargo metadata --locked
cargo fmt --check
cargo check --locked
cargo test --locked
```

跨仓库职责见 [docs/repository-boundaries.md](docs/repository-boundaries.md)。
