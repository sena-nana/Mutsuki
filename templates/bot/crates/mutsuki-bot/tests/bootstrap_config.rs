use std::path::Path;

use mutsuki_bot::load_bootstrapped_product;
use mutsuki_bot_web_console::PRODUCT_CONFIG_PROVIDER_ID;
use mutsuki_config_service::{ConfigContext, capability};

fn bootstrap(root: &Path, extra: &str) -> std::path::PathBuf {
    let path = root.join("bootstrap.toml");
    std::fs::write(
        &path,
        format!(
            r#"
[host]
instance_id = "test"
home_dir = "."
data_dir = "data"

[security]
secret_file = "secrets.toml"

[config_repository]
repository_plugin_id = "mutsuki.config.repository.sqlite"
document_namespace = "test"
[config_repository.options]
path = "config.sqlite3"
{extra}
"#
        ),
    )
    .unwrap();
    path
}

#[tokio::test]
async fn empty_sqlite_is_seeded_once_and_restored() {
    let root = tempfile::tempdir().unwrap();
    let path = bootstrap(root.path(), "");
    let first = load_bootstrapped_product(&path).await.unwrap();
    let first_product = first
        .config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .unwrap();
    assert_eq!(first_product.revision.0, 1);
    let enabled = first
        .service
        .plugins
        .configured
        .iter()
        .filter(|selection| selection.enabled)
        .map(|selection| selection.id.as_str())
        .collect::<Vec<_>>();
    assert!(enabled.is_empty());
    assert_eq!(first.console.extensions, vec!["config"]);

    drop(first);
    let second = load_bootstrapped_product(&path).await.unwrap();
    let second_product = second
        .config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .unwrap();
    assert_eq!(second_product.revision, first_product.revision);
}

#[tokio::test]
async fn legacy_monolithic_fields_are_rejected_by_bootstrap() {
    let root = tempfile::tempdir().unwrap();
    let path = bootstrap(root.path(), "\n[service]\nprofile = \"bot\"");
    let error = match load_bootstrapped_product(&path).await {
        Ok(_) => panic!("legacy bootstrap unexpectedly loaded"),
        Err(error) => error,
    };
    assert!(error.contains("unknown field"));
}

#[tokio::test]
async fn unknown_repository_plugin_fails_before_runtime_load_plan() {
    let root = tempfile::tempdir().unwrap();
    let path = bootstrap(root.path(), "");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("mutsuki.config.repository.sqlite", "example.remote");
    std::fs::write(&path, text).unwrap();
    let error = match load_bootstrapped_product(&path).await {
        Ok(_) => panic!("unknown repository unexpectedly loaded"),
        Err(error) => error,
    };
    assert!(error.contains("unknown bootstrap repository plugin"));
}
