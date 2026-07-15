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

## 可选 DistributedHost

提交的产品配置显式使用 `[distribution] mode = "disabled"`。此模式不会启动或监管
DistributedHost，不打开分布式网络，不产生分布式遥测或副本任务；本地 ServiceRuntime 的行为
与未接入分布式时一致。

需要分布式时，由部署系统单独安装和监管 `mutsuki-distributed-host`，产品配置只引用一个相对
部署文件：

```toml
[distribution]
mode = "clustered"
deployment = "../deploy/distribution/controller-worker.toml"
acceptance = "fast"
fallback = "reject"
```

仓库提供单机、单控制端+Worker、3 投票节点+Worker 三种机器中立拓扑以及任务策略 catalog。
模板在启动本地 Runtime 前验证固定 release/revision、Secret key 引用、拓扑、认证加密通道、
CPU/内存/显存/网络/并发/checkpoint 预算以及策略文件，并使用 Host secret 边界连接已运行的
sidecar，认证校验 capability schema/protocol、revision、maturity、feature proof 和 health。
模板不会启动、重启或监管 sidecar，也不会链接 scheduler/recovery 实现。仅 Fast 可显式选择
`local_degraded`，该状态会进入 ServiceHost health；Durable/Critical 必须拒绝本地回退，不能
伪装成可靠接收。当前 HA/Durable/Critical/checkpoint/trust 未达到 deployable maturity 时会
返回 `ExperimentalUnavailable`。控制通道与直接数据通道独立，大型数据不经过 Leader。

部署、健康状态、故障演练和诊断见
[docs/distributed-deployment.md](docs/distributed-deployment.md)；跨阶段总验收矩阵见
[docs/distributed-acceptance.md](docs/distributed-acceptance.md)。

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

## Bilibili 本地装配

模板链接 StdPlugins 与 BotPlugins 的 owner catalog，但提交的 `config/template.toml` 仍保持
零插件。Bilibili 本地配置必须用 `backend.type` 显式选择 `web_cookie` 或
`open_platform`，不会在两者之间静默 fallback。Web backend 的 Cookie 值只写入本地 secret
文件：

```toml
[secrets]
BILIBILI_COOKIE = "SESSDATA=replace-locally"
```

官方开放平台 backend 只复用授权账号的直播与已发布稿件轮询协议；动态、链接解析、Cookie
扫码/管理和 Chromium 352 路径都不会被声明或替代。产品配置保存 `client_id`、授权 UID 与
两个 Host secret key 引用，app secret 和可原子刷新的 OAuth bundle 只写入本地 secret 文件：

```toml
[secrets]
BILIBILI_OPEN_APP_SECRET = "replace-locally"
BILIBILI_OPEN_OAUTH = '''{"access_token":"replace-locally","refresh_token":"replace-locally","expires_at":1893456000,"scopes":["LIVE_ROOM_DATA","ARC_BASE"]}'''
```

完整配置、scope 和错误模型见
[BotPlugins 官方开放平台 backend](https://github.com/sena-nana/MutsukiBotPlugins/blob/0feead21eab479d2944225648f62002cd216af79/docs/bilibili-open-platform.md)。

要启用图片资源，产品还需显式选择 `mutsuki.std.resource.memory`（或另一个兼容 owner
Provider），并让 QQ 的 `media_provider_id` 与业务插件一致。米画师还需显式选择
`mutsuki.std.io.browser.chromium`；其 `executable` 必须在本地配置中填写，仓库不提交任何
机器路径。浏览器 allowlist 应仅包含实际产品需要的米画师域名。

迁移版只使用原始封面/头像资源，不生成 HTML 卡片截图。Cookie 扫码登录、聊天管理/自助
绑定、暂停/预览和 Bilibili 352 浏览器路径属于显式 `web_cookie` backend；官方 backend
拒绝这些 Web-only 配置。

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
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

跨仓库职责见 [docs/repository-boundaries.md](docs/repository-boundaries.md)。
