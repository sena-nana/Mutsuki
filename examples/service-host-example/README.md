# Service Host Example

Runs the QQBot echo smoke through the current MutsukiCore `RuntimeBootstrapper` host-side boot entry:

```powershell
cargo run -p service-host-example
```

This example does not define a `BotHost`. It registers the QQBot adapter, event router, command parser, and echo business runner as builtin batch-first plugins, submits a one-entry QQBot gateway batch, and lets the runtime drive the resulting tasks until idle.

It reads the same smoke configuration variables as `qqbot-echo`: `QQBOT_ACCOUNT_ID`, `QQBOT_APP_ID`, `QQBOT_CLIENT_SECRET`, `QQBOT_GROUP_OPENID`, `QQBOT_USER_OPENID`, and `QQBOT_ECHO_TEXT`. The HTTP transport remains a recording client for the smoke run, so no QQBot network request is sent.
