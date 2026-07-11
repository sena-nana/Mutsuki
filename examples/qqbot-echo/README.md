# QQBot Echo 模拟运行说明

在本地运行不需要真实凭据的 QQBot recording smoke：

```powershell
cargo run -p qqbot-echo
```

该程序使用固定假数据提交一个 `GROUP_MESSAGE_CREATE` Gateway frame，并经过以下标准任务链路：

```text
QQBot Gateway frame
  -> QQBot Adapter Gateway Runner
  -> mutsuki.bot.event/ingest@1
  -> Bot Event Router
  -> mutsuki.bot.command/parse@1
  -> Bot Command Plugin
  -> 平台无关的 Echo 业务插件
  -> mutsuki.bot.message/send@1
  -> QQBot Adapter OpenAPI Runner
```

链路中的所有 Runner 都使用 MutsukiCore `run_batch`。OpenAPI client 是 recording client，
不会发出真实网络请求；程序输出的是脱敏后的请求记录。

本示例不读取环境变量或本地配置。真实账号配置由具体产品 Host 通过其配置能力构造，
再将已构造的 `QqBotConfig` 交给 `QqBotPluginBundle`。
