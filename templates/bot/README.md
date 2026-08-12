# MutsukiBotTemplate

配置驱动、实现中立的 Mutsuki Bot 产品装配器。模板只选择 bootstrap provider、聚合 owner
catalog 并启动 ServiceRuntime；它不实现命令、回复、Agent 流程或业务 Bot。

唯一规范源位于主仓 `templates/bot`。独立的 `sena-nana/MutsukiBotTemplate` 由 release 自动
导出并固定统一 tag/commit，禁止在生成仓手工维护实现或依赖版本。

## 启动

`config/bootstrap.toml` 是最小 bootstrap，只包含 Host identity/directories、secret、插件发现
以及配置仓库选择。模板显式选择 SQLite；框架和 ConfigService 不假设存储位置。

```powershell
Copy-Item config/bootstrap.toml config/local.toml
Copy-Item config/secret.template.toml config/local.secret.toml
cargo run -p mutsuki-bot -- config/local.toml
```

路径优先级为 CLI、`MUTSUKI_BOOTSTRAP`、`config/bootstrap.toml`。旧完整产品 TOML 会因未知
字段被拒绝，不自动导入旧配置或旧 Flow 数据。

空仓库只以 revision CAS 写入一次版本化种子：

- 不启用任何 Runtime 插件；
- 预声明但不启用 Agent connection、Flow Router、QQ 与 Bot Agent bridge；
- 启用仅监听 `127.0.0.1:8787` 的鉴权 Console，且只选择通用配置页面。

首次进入配置页后，启用“本机 Bot 工作区”并保存；应用提示重启后才装配 Agent connection、
Flow Router 及对应管理页面。QQ、模型和 Bot Agent 仍分别由各自配置页启用，系统不会自动生成
Flow。

已有 document 永不被种子覆盖。产品插件选择、WebExtension 选择以及各 owner 配置均保存到
配置仓库，而不是写回 bootstrap。Secret 明文只进入 Host secret store，配置文档保存引用或
脱敏状态。首次启动会在 Git 忽略的 `config/local.secret.toml` 创建随机 Console Token，并在
Unix 平台限制为当前用户可读写；QQ 与模型密钥只能在 Web 中写入，之后不会回显。

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

模板只聚合 Std、Agent 和 Bot owner 的 configured factory catalog。零插件配置可启动为空闲
Runtime；显式选择但缺失 factory、capability、binding、secret 或不兼容 Flow 节点时必须失败。
Core 和通用 Host 只理解领域中立 `PluginExtensionDescriptor`、LoadPlan 与 ConfigService 事务，
不解码 Bot Flow。

QQ、Agent、Bilibili、Media、Interaction、Delivery 与业务插件的配置和运行规则属于各 owner。
BotTemplate 不提供生产 fallback。真实 QQ 账号 smoke 仍需本地凭据并单独运行；确定性验收使用
fake HTTP/WebSocket 边界。

## 导出与验证

```powershell
python templates/bot/scripts/export_template.py `
  --output ../MutsukiBotTemplate-export `
  --tag v0.2.0
```

导出后必须在无兄弟仓库目录中运行 locked metadata、fmt/check/test 和模板 smoke，确认生成树
不依赖主仓 path 或本地账号数据。
