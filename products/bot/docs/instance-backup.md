# 单实例备份与恢复

第一方 `mutsuki-bot` 只使用可执行文件旁的 `.mutsuki-bot/` 作为实例根（`INSTANCE_DIR`）。产品不提供备份 CLI、SQLite backup API 或 WAL checkpoint 入口。

源码运行时实例根是 `target/debug/.mutsuki-bot/`（或对应 `target/release/`）。目录不可写、端口占用或重复启动会直接失败，不回退到其他位置。

## 实例内容

| 路径 | 职责 |
| --- | --- |
| `config.sqlite3` | `ConfigRepository`（namespace `mutsuki-bot`）：产品、owner、Flow 文档。打开时 `PRAGMA journal_mode = WAL`。 |
| `secrets.toml` | Host secret 文件。启动先调用 `recover_host_secret_transaction`。Unix 上仅当前用户可读写。 |
| `data/bot/state.sqlite3` | `BotStateDb`：会话、投递、交互、persona、conversation-context、沙盒历史。WAL、`busy_timeout` 5s、`synchronous=NORMAL`；owner catalog 共享一个 actor。 |
| `data/bilibili/state.sqlite3` | B 站 cursor、未完成 QR / 绑定 challenge、cooldown。不是订阅关系权威（订阅在配置仓库 + Host secret）。 |
| `data/agent/local/state.sqlite3` | Local Agent 会话。WAL。 |
| `data/resources.sqlite` | 媒体 `ResourceRef` 字节。retention 24 小时 / 512 MiB。WAL、`busy_timeout` 5s、`synchronous=NORMAL`。 |
| `instance.boundary` | 启动时作为 `ServiceConfig::finalize_bootstrap` 的边界路径；当前实现不写入该文件。 |
| `logs/`、`run/`、`plugins/` | 日志、运行时（含 `run/control.token`）、动态插件 `installed` / `disabled`。 |
| `fonts/` | 控制台渲染字体；缺失时启动会从产品资源重新安装。 |

WAL 库在运行中可能还有同名 `-wal`、`-shm`。Host secret 若正处在协调写入，还可能短暂出现 `secrets.mutsuki-secret-transaction.toml` 及其 `.commit` 标记；启动恢复会消费它们。不要在文档、日志或备份说明里写入 secret 明文。

`logs/` 与 `run/` 对业务状态不是必需；恢复后进程会重建运行时文件。`plugins/` 只在该实例实际安装了动态插件时才需要一并带走。

## SQLite 耐久性

`data/bot/state.sqlite3`（以及同样 pragma 的 `data/resources.sqlite`）使用 WAL + `synchronous=NORMAL`：电源故障时最后若干已提交事务可能丢失。这是有意取舍，不是缺陷。

两个进程不得同时打开同一份 `state.sqlite3`。产品装配对每个实例只打开一个 actor 并在 Sandbox / QQ / Agent / Interaction 之间共享；不要对同一实例目录再启动第二个 `mutsuki-bot`，也不要另开工具进程读写该文件。

对正在运行的 WAL 库做普通文件拷贝（即使同时拷了 `-wal`/`-shm`）也可能得到损坏副本。没有产品侧 backup API 可调用。

## 备份

推荐做法：先停止 `mutsuki-bot`，再复制整个实例目录。WAL 库若存在 `-wal`/`-shm`，必须与主文件一起复制。

```powershell
# 先确认 mutsuki-bot 已退出，再复制实例根
Copy-Item -Recurse .mutsuki-bot backup\mutsuki-bot
```

若必须在进程仍运行时备份，使用本机 `sqlite3` 的 `.backup` 或 `VACUUM INTO` 生成一份独立数据库文件；这是 SQLite 工具，不是产品命令，也不要发明产品 CLI 开关。不要对活动 WAL 文件做资源管理器 / `Copy-Item` 式拷贝。

## 恢复

1. 停止 `mutsuki-bot`。
2. 用备份替换实例目录中的对应文件（至少包括配置库、secret 文件和 `data/` 下各 SQLite；WAL 库带上 `-wal`/`-shm`）。
3. 再启动。空配置仓库只会以 CAS 写入一次版本化种子（产品文档、工作区选择、无 Flow 记录时的 `qq.business.full`）；已有 Flow / owner 文档永不覆盖，包括用户主动清空后的空图。

旧 `local.toml`、旧 bootstrap、旧 SQLite schema 和旧 secret 布局不读取、不迁移。当前 `mutsuki.product` 为 schema/value v3：不兼容的配置库以 `product.config.version_unsupported` 拒绝启动，Secret 文件不会被自动删除。
