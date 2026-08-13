use mutsuki_bot::{load_bootstrapped_product, run_bootstrapped_product};
use std::ffi::OsString;
use std::path::PathBuf;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bootstrap_path = select_bootstrap_path(
        std::env::args_os().nth(1),
        std::env::var_os("MUTSUKI_BOOTSTRAP"),
    );
    let product = load_bootstrapped_product(&bootstrap_path).await?;
    run_bootstrapped_product(product).await?;
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
    fn bootstrap_path_precedence_is_cli_then_environment_then_product_default() {
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
