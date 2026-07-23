//! MVP: Discord-like plugin config with #[derive(MutsukiConfig)] + Web console.

use std::sync::Arc;

use mutsuki_bot_config::{
    ConfigApplyMode, ConfigProviderRegistry, ConfigService, ConfigValue, MemoryConfigProvider,
    MutsukiConfig, MutsukiConfigSchema,
};
use mutsuki_plugin_bot_config_web::{ConfigWebExtension, materialize_frontend_assets};
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost};
use mutsuki_web_protocol::{DeploymentMode, WebApplicationDescriptor, WebShellAssets};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, MutsukiConfig)]
#[config(provider_id = "discord", title = "Discord Adapter")]
pub struct DiscordConfig {
    #[config(title = "Bot Token", secret, required)]
    pub token: String,

    #[config(title = "指令前缀", default = "/")]
    pub command_prefix: String,

    #[config(title = "自动重连")]
    pub auto_reconnect: bool,

    #[config(
        title = "重连间隔",
        unit = "秒",
        min = 1,
        max = 300,
        visible_if = "auto_reconnect == true",
        restart = "plugin_reload"
    )]
    pub reconnect_interval: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = Arc::new(ConfigProviderRegistry::default());
    let defaults = ConfigValue::Object(
        [
            (
                "token".into(),
                ConfigValue::Secret(mutsuki_bot_config::SecretState::Absent),
            ),
            ("command_prefix".into(), ConfigValue::String("/".into())),
            ("auto_reconnect".into(), ConfigValue::Bool(true)),
            ("reconnect_interval".into(), ConfigValue::Integer(5)),
        ]
        .into_iter()
        .collect(),
    );
    let provider = Arc::new(MemoryConfigProvider::new(
        DiscordConfig::schema(),
        defaults,
        ConfigApplyMode::HotReload,
    ));
    registry.register(provider)?;
    let service = Arc::new(ConfigService::new(registry));

    let assets_dir = tempfile::tempdir()?;
    let shell_dir = tempfile::tempdir()?;
    let frontend = materialize_frontend_assets(assets_dir.path())?;
    let extension = ConfigWebExtension::new(service).with_frontend_assets(&frontend);

    let shell = WebShellAssets {
        root_dir: frontend.clone(),
        index_file: "index.html".into(),
        import_map: Default::default(),
    };
    let app = MinimalWebApplication::new(
        WebApplicationDescriptor {
            id: "mutsuki.bot.config.console".into(),
            name: "Mutsuki Config Console".into(),
            version: "0.1.0".into(),
            brand: Some("Mutsuki".into()),
            theme: Some("lilia".into()),
        },
        shell,
    );

    let listen =
        std::env::var("MUTSUKI_CONFIG_DEMO_LISTEN").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let mut host = MutsukiWebHost::builder()
        .application(app)
        .listen(&listen)
        .mode(DeploymentMode::Embedded)
        .shell_dir(shell_dir.path())
        .extension(extension)
        .auth_token("local-dev")
        .build()?;
    host.start().await?;
    // Keep tempdirs alive for process lifetime.
    std::mem::forget(assets_dir);
    std::mem::forget(shell_dir);
    println!(
        "Mutsuki config console listening on http://{} (schema-first Discord demo)",
        host.listen_addr().unwrap()
    );
    tokio::signal::ctrl_c().await?;
    host.stop().await?;
    Ok(())
}
