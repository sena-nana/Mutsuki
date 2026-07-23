# Standalone Bot Console（分进程部署）

产品提供两个可发布进程：

| 进程 | Binary | 职责 |
| --- | --- | --- |
| Runtime | `mutsuki-bot` | ServiceRuntime；可选嵌入式 Console（同进程） |
| Console | `mutsuki-bot-console` | 独立 WebHost Console，经 Link 控制已运行的 Runtime |

业务页面仍由 BotPlugins WebExtension 提供；WebHost 只做宿主，Recovery Shell 不会变成 Console。

## 何时用独立 Console

- Runtime 与 Console 分机或分进程部署
- 远程管理：Console 通过 `quic://` 连接 Runtime 的 Link 控制面
- 本机分进程：Console 通过 `local://mutsuki.servicehost` 连接同机 Runtime

同进程嵌入式 Console 继续用 `mutsuki-bot` + `[web.console] enabled = true`（不设 `link_endpoint`）。

## 启动顺序

1. 准备被 Git 忽略的 `config/local.toml` 与 `config/local.secret.toml`（从模板复制）。
2. 启动 Runtime（开启 Link；独立 Console 不要在 Runtime 侧再开嵌入式 Console，或接受双实例）：

   ```bash
   cargo run -p mutsuki-bot -- config/local.toml
   ```

3. 另起 Console 进程（同一份或专用 console 配置均可；必须 `enabled = true` 且设置 `link_endpoint`）：

   ```bash
   cargo run -p mutsuki-bot --bin mutsuki-bot-console -- config/local.toml
   ```

配置路径优先级与 Runtime 相同：CLI → `MUTSUKI_CONFIG` → 仓库 `config/local.toml`。

## 配置要点

```toml
# Runtime：开启 QUIC Link（secret 只写 key；PEM 在 local.secret.toml）
[link.quic]
enabled = true
listen = "127.0.0.1:4433"
cert_pem_key = "LINK_QUIC_CERT_PEM"
key_pem_key = "LINK_QUIC_KEY_PEM"

# Console 进程：Standalone + Link
[web.console]
enabled = true
listen = "127.0.0.1:8787"
auth_token_key = "WEB_CONSOLE_AUTH_TOKEN"
link_endpoint = "quic://127.0.0.1:4433"
quic_server_name = "localhost"
quic_ca_cert_key = "LINK_QUIC_CA_CERT_PEM"
```

本机分进程也可使用：

```toml
link_endpoint = "local://mutsuki.servicehost"
```

此时 Runtime 需启用 IPC / Link 本机控制桥（`[ipc] enabled = true`），Console 侧不必配置 `quic_*`。

`mutsuki-bot-console` 在缺少 `link_endpoint`、缺少 QUIC TLS secret、或 `enabled = false` 时结构化失败，不会假成功。

## 验证

- 库装配：`standalone_quic_smoke`（同进程内 `build_standalone_console_from_product`）
- 进程面：`standalone_console_process_smoke`（真实 `mutsuki-bot-console` 二进制 + QUIC + `control.health`）
