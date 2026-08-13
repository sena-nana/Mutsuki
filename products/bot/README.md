# Mutsuki Bot

配置驱动、实现中立的第一方 Mutsuki Bot 产品。产品入口只选择 bootstrap provider、聚合 owner
catalog 并启动 ServiceRuntime；它不实现命令、回复、Agent 流程或业务 Bot。源码、运行入口、
Issue 和发布基线均位于 Mutsuki 主仓 `products/bot`。

## 启动

产品只支持一个本地实例，不读取配置路径、profile、namespace 或 `MUTSUKI_BOOTSTRAP`：

```powershell
cargo run --locked -p mutsuki-bot
```

运行目录固定在可执行文件旁的 `.mutsuki-bot/`。源码运行时即
`target/debug/.mutsuki-bot/`，其中包含 `config.sqlite3`、`secrets.toml`、`data/`、`logs/`、
`run/` 和 `plugins/{installed,disabled}/`。目录不可写、端口占用或重复启动会直接失败，不回退
到其他位置。

首次交互启动会隐藏输入并确认管理台口令；非交互部署必须设置
`MUTSUKI_SECRET_MUTSUKI_WEB_CONSOLE_TOKEN`。口令只进入权限受限的 Host secret 文件，不打印、
不写日志、不进入 SQLite。

空仓库只以 revision CAS 写入一次版本化种子：

- 直接启用 Agent Connections 与 Flow Router；
- 直接提供 Config、QQ、Agent 和 Bot Flow 管理页面，无首次重启步骤；
- QQ、Local Agent 与 Bot Agent bridge 仍保持关闭，不生成业务 Flow；
- 鉴权 Console 固定监听 `127.0.0.1:8787`。

旧 `local.toml`、旧 bootstrap、旧 SQLite 和旧 secret 不读取、不迁移；升级后需在新实例中重新
配置。QQ、模型和 Bot Agent 分别由各自配置页启用，系统不会自动生成 Flow。

已有 document 永不被种子覆盖。产品插件选择、WebExtension 选择以及各 owner 配置均保存到
配置仓库，而不是写回 bootstrap。Secret 明文只进入 Host secret store，配置文档保存引用或
脱敏状态。Unix 平台的 `.mutsuki-bot/secrets.toml` 仅允许当前用户读写；QQ 与模型密钥只能在
Web 中写入，之后不会回显。

当前 `mutsuki.product` 使用 schema/value v3。v3 是破坏性配置边界：旧版本 SQLite 产品文档
会以 `product.config.version_unsupported` 拒绝启动，部署者必须重建配置仓库；Secret 文件不会
被自动删除。`workspace_enabled` 唯一选择 Agent Connections、Flow Router 和对应 Console 页面，
QQ、Local Agent、Bot Agent 的启用状态与配置唯一来自各自 owner document；`runtime_plugins`
只允许选择其余通用插件，不能重复声明上述 owner 插件。

## Bot Flow

Bot Flow 是 provider id 为 `mutsuki.bot.flow` 的普通配置文档。插件仅声明
`mutsuki.bot.flow.nodes@1` 节点、类型化端口、schema 和精确 binding；匹配、顺序、命令与分支
全部来自 active Flow snapshot。

`bot-flow-editor` 是独立、显式选择的 WebExtension；默认种子不选择它，也不会生成 Flow：

- Router 与编辑器仍保持独立 owner 边界；
- 浏览器本地保存未提交草稿并绑定读取 revision；
- RPC 只有 `catalog.read`、`snapshot.read`、`validate`、`apply`；
- apply 通过 ConfigService 一次 CAS，冲突保留候选且不自动合并。

Flow v1 支持无环分支、显式扇出和 error edge，不支持循环或隐式 join。在途 task 持有旧的
不可变 graph revision，配置更新不会改变已开始的执行。

## 装配边界

产品只聚合 Std、Agent 和 Bot owner 的 configured factory catalog。零插件配置可启动为空闲
Runtime；显式选择但缺失 factory、capability、binding、secret 或不兼容 Flow 节点时必须失败。
Core 和通用 Host 只理解领域中立 `PluginExtensionDescriptor`、LoadPlan 与 ConfigService 事务，
不解码 Bot Flow。

QQ、Agent、Bilibili、Media、Interaction、Delivery 与业务插件的配置和运行规则属于各 owner。
产品不提供生产 fallback。真实 QQ 账号 smoke 仍需本地凭据并单独运行；确定性验收使用
fake HTTP/WebSocket 边界。

## 验证

```powershell
python skills/monorepo-maintenance/scripts/check_workspace.py
cargo metadata --locked --format-version 1
cargo test --locked -p mutsuki-bot --all-targets
```

依赖或产品装配变更还必须在无兄弟仓库的主仓 clone 中运行 locked metadata、fmt/check/test 和
产品 smoke，确认运行入口不依赖本机账号数据或仓库外路径。
