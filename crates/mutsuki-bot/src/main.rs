use std::ffi::OsString;
use std::path::PathBuf;

use mutsuki_bot::{assemble_service, load_distribution_plan, repository_local_config_path};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = select_config_path(
        std::env::args_os().nth(1),
        std::env::var_os("MUTSUKI_CONFIG"),
    );
    // Validate the external deployment contract before starting the local
    // ServiceRuntime. The template never starts or supervises the sidecar.
    let _distribution = load_distribution_plan(&config_path)?;
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path),
        ..Default::default()
    })?;
    assemble_service(service)?
        .start()
        .await?
        .run_foreground()
        .await?;
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
