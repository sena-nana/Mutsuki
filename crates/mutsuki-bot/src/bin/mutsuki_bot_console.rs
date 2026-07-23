//! Standalone Bot Console process: WebHost + Link control bridge.
//!
//! Runs separately from the ServiceRuntime product (`mutsuki-bot`). Requires
//! `[web.console] enabled = true` and a non-empty `link_endpoint`
//! (`local://mutsuki.servicehost` or `quic://host:port` with TLS secret refs).

use std::ffi::OsString;
use std::path::PathBuf;

use mutsuki_bot::{
    build_standalone_console_from_product, load_web_console_config, repository_local_config_path,
};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_web_host::WebHost;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = select_config_path(
        std::env::args_os().nth(1),
        std::env::var_os("MUTSUKI_CONFIG"),
    );
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path.clone()),
        ..Default::default()
    })?;
    let console = load_web_console_config(&config_path)?;
    if !console.enabled {
        return Err("web.console.enabled must be true for mutsuki-bot-console".into());
    }
    if console
        .link_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Err(
            "standalone console requires web.console.link_endpoint (local:// or quic://)".into(),
        );
    }

    let (mut host, _assets) = build_standalone_console_from_product(&config_path, &service)?;
    host.start().await?;
    if let Some(addr) = host.listen_addr() {
        eprintln!("Mutsuki Bot Console (standalone) listening on http://{addr}");
    } else {
        return Err("standalone console started without a listen address".into());
    }

    tokio::signal::ctrl_c().await?;
    host.stop().await?;
    Ok(())
}

fn select_config_path(cli: Option<OsString>, environment: Option<OsString>) -> PathBuf {
    cli.or(environment)
        .map(PathBuf::from)
        .unwrap_or_else(repository_local_config_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_precedence_is_cli_then_environment_then_repository_local() {
        assert_eq!(
            select_config_path(Some("cli.toml".into()), Some("env.toml".into())),
            PathBuf::from("cli.toml")
        );
        assert_eq!(
            select_config_path(None, Some("env.toml".into())),
            PathBuf::from("env.toml")
        );
        assert_eq!(
            select_config_path(None, None),
            repository_local_config_path()
        );
    }
}
