# Schema-first 插件配置协议

本仓库实现 [Issue #15](https://github.com/sena-nana/MutsukiBotPlugins/issues/15)：

- `mutsuki-bot-config`：ConfigDescriptor / ConfigProvider / revision / secret / expr
- `mutsuki-bot-config-derive`：`#[derive(MutsukiConfig)]`
- `mutsuki-plugin-bot-config-web`：默认 Web 配置插件（`config.*` RPC + Koishi 风格控制台 + LiliaUI tokens）

普通插件只需：

```rust
#[derive(MutsukiConfig)]
#[config(provider_id = "discord", title = "Discord")]
struct DiscordConfig {
    #[config(title = "Bot Token", secret, required)]
    token: String,
}
```

然后 `registry.register(Arc::new(MemoryConfigProvider::new(...)))`。
不需要 Vue/Node/Vite/WebHost 依赖。

## MVP 验证

```bash
cargo test -p mutsuki-bot-config --test protocol
cargo test -p mutsuki-plugin-bot-config-web --test web_e2e
cargo bench -p mutsuki-bot-config --bench config_perf
cargo run -p config-demo
```

浏览器打开控制台后可列出 provider、自动生成表单、验证并 apply（含 secret keep/set/clear 与 revision）。
