use std::ffi::OsString;
use std::path::PathBuf;

use mutsuki_bot::{assemble_service, prepare_distribution, repository_local_config_path};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};

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
    // The template authenticates and proves an already-running sidecar; it
    // never starts, restarts, or supervises that external process.
    let mut distribution = prepare_distribution(&config_path, &service).await?;
    let builder = distribution.attach_health_probe(assemble_service(service)?);
    let _monitor = distribution.start_monitor();
    builder.start().await?.run_foreground().await?;
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
