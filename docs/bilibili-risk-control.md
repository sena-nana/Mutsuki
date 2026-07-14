# Bilibili 352 Chromium backend

Bilibili 动态 Web API 返回 352 时，产品可以显式选择 Chromium backend。默认配置不回退，
仍以 `bilibili.risk_control_352` 失败，并在 error evidence 中报告
`fallback_status = not_configured` 与 `degraded = true`。

产品必须同时选择通用 Chromium provider 和 Bilibili owner plugin：

```toml
[[plugins.configured]]
id = "mutsuki.std.io.browser.chromium"

[plugins.configured.config]
executable = "/absolute/path/to/chromium"
domain_allowlist = ["bilibili.com"]
timeout_ms = 15000
max_dom_bytes = 2097152

[[plugins.configured]]
id = "mutsuki.bot.bilibili"

[plugins.configured.config]
# Other required Bilibili product fields are omitted here.
risk_control = { backend = "chromium", timeout_ms = 10000, max_response_bytes = 2097152 }
```

`risk_control` 缺失时不会调用 browser protocol。显式选择后，Bilibili manifest 要求
`task_protocol:mutsuki.browser.snapshot`，因此产品漏选 provider 会在 RuntimeLoadPlan
阶段失败，而不是运行中静默改走其他路径。

Chromium provider 只允许 HTTPS 和 `domain_allowlist` 中的 DNS domain，并在启动时检查
executable 是绝对文件。provider 的 `timeout_ms` 与 `max_dom_bytes` 是上限；Bilibili
请求的 timeout 不得超过 provider 上限，读取 snapshot 后还会再次检查
`max_response_bytes`。最终重定向 URL 与动态卡片、图片 URL 都必须属于 Bilibili/HDslb
allowlist。

成功回退会完成原 poll task，并追加
`mutsuki.bot.bilibili.risk_control/status@1` event，payload 包含 backend、352、
`status = degraded` 和 `fallback = succeeded`。browser child task、resource 读写、DOM
解析、越域或大小检查失败时返回 `bilibili.risk_control_fallback_failed`，evidence 包含
backend、fallback_status、degraded、risk_control_code 与 detail。

验证层级：

- 单元/Runner 测试使用 352 transport、browser child task outcome 与 snapshot resource
  fake，保留 AsyncRunnerAdapter、TaskAwait、ResourceRef 和 ResultRouter 输入路径。
- 产品装配测试验证漏选 browser protocol 和缺失 Chromium artifact 会在启动阶段失败。
- 真实 Chromium smoke 需要本地 `CHROMIUM_EXECUTABLE` 与网络，不属于默认 CI；未执行时
  不得把 fake 测试称为真实 smoke。
