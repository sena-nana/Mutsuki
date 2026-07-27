# 平台无关 Echo 业务插件

该示例业务插件只依赖 `mutsuki.bot.*` 协议和 `mutsuki-bot-sdk::MessageBuilder`。
它处理 `/echo` 与 `/ping` 命令，并生成标准 `mutsuki.bot.message/send@1` 任务，
不依赖 QQBot Adapter、HTTP client、WebSocket 或 ServiceHost。

编译验证：

```powershell
cargo test -p bot-echo
```
