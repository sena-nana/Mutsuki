use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use mutsuki_bot::{WebConsoleGuard, assemble_service_with_connections, load_bootstrapped_product};
use mutsuki_bot_web_console::TargetedPluginReloadLifecycle;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bootstrap_path = select_bootstrap_path(
        std::env::args_os().nth(1),
        std::env::var_os("MUTSUKI_BOOTSTRAP"),
    );
    let product = load_bootstrapped_product(&bootstrap_path).await?;
    let runtime = assemble_service_with_connections(
        product.service.clone(),
        product.config.clone(),
        product.agent_connections.clone(),
    )?
    .start()
    .await?;
    product
        .config
        .set_lifecycle(Arc::new(TargetedPluginReloadLifecycle::new(
            runtime.handle(),
        )));
    let console = WebConsoleGuard::start(
        product.console,
        &product.root,
        &product.service,
        &runtime,
        product.config,
    )
    .await?;
    if let Some(console) = &console
        && let Some(addr) = console.listen_addr()
    {
        eprintln!("Mutsuki Web Console listening on http://{addr}");
    }
    let result = runtime.run_foreground().await;
    if let Some(console) = console {
        console.stop().await?;
    }
    result?;
    Ok(())
}

fn select_bootstrap_path(cli: Option<OsString>, environment: Option<OsString>) -> PathBuf {
    cli.or(environment).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/bootstrap.toml")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_path_precedence_is_cli_then_environment_then_template() {
        assert_eq!(
            select_bootstrap_path(Some("cli.toml".into()), Some("env.toml".into())),
            PathBuf::from("cli.toml")
        );
        assert_eq!(
            select_bootstrap_path(None, Some("env.toml".into())),
            PathBuf::from("env.toml")
        );
        assert!(select_bootstrap_path(None, None).ends_with("config/bootstrap.toml"));
    }
}
