# QQBot Echo Smoke

Runs a local QQBot smoke path without real credentials:

```powershell
cargo run -p qqbot-echo
```

Optional smoke configuration is read from environment variables:

```powershell
$env:QQBOT_ACCOUNT_ID="example-bot"
$env:QQBOT_APP_ID="APP_ID"
$env:QQBOT_CLIENT_SECRET="CLIENT_SECRET"
$env:QQBOT_GROUP_OPENID="GROUP_OPENID"
$env:QQBOT_USER_OPENID="USER_OPENID"
$env:QQBOT_ECHO_TEXT="/echo hello from qqbot"
cargo run -p qqbot-echo
```

The executable submits one QQBot gateway `GROUP_MESSAGE_CREATE` frame, routes it through:

```text
QQBot gateway frame
  -> mutsuki-plugin-bot-adapter-qqbot gateway runner
  -> mutsuki.bot.event/ingest@1
  -> mutsuki-plugin-bot-event-router
  -> mutsuki.bot.command/parse@1
  -> mutsuki-plugin-bot-command
  -> echo command handler
  -> mutsuki.bot.message/send@1
  -> mutsuki-plugin-bot-adapter-qqbot OpenAPI runner
```

The OpenAPI client is a recording client, so the smoke is deterministic and does not send network traffic. A production host should provide real config and transport when registering the QQBot adapter runner.
