use std::path::Path;

use mutsuki_bot::PRODUCT_CONFIG_PROVIDER_ID;
use mutsuki_bot::load_bootstrapped_product;
use mutsuki_config_service::{
    ConfigApplyRequest, ConfigCompareAndSetRequest, ConfigContext, ConfigDocumentKey, ConfigValue,
    capability,
};

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
    assert_eq!(first_product.schema_version, 3);
    assert_eq!(first_product.value_version, 3);
    assert!(
        first_product
            .value
            .as_object()
            .unwrap()
            .get("extensions")
            .is_none()
    );
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
async fn legacy_product_document_version_is_rejected_without_migration() {
    let root = tempfile::tempdir().unwrap();
    let path = bootstrap(root.path(), "");
    let first = load_bootstrapped_product(&path).await.unwrap();
    let snapshot = first
        .config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .unwrap();
    let mut write = first
        .config
        .repository()
        .prepare_compare_and_set(ConfigCompareAndSetRequest {
            key: ConfigDocumentKey::new(PRODUCT_CONFIG_PROVIDER_ID, ConfigContext::global()),
            expected_revision: snapshot.revision,
            value: snapshot.value,
            schema_version: 2,
            value_version: 2,
        })
        .unwrap();
    write.commit().unwrap();
    write.finish().unwrap();
    drop(first);

    let error = match load_bootstrapped_product(&path).await {
        Ok(_) => panic!("legacy product document unexpectedly loaded"),
        Err(error) => error,
    };
    assert!(error.contains("product.config.version_unsupported"));
}

#[tokio::test]
async fn owner_plugin_ids_are_rejected_from_runtime_plugins() {
    let root = tempfile::tempdir().unwrap();
    let path = bootstrap(root.path(), "");
    let first = load_bootstrapped_product(&path).await.unwrap();
    let snapshot = first
        .config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .unwrap();
    let ConfigValue::Object(mut candidate) = snapshot.value else {
        panic!("product config must be an object");
    };
    candidate
        .get_mut("runtime_plugins")
        .and_then(ConfigValue::as_object_mut)
        .unwrap()
        .insert(
            "mutsuki.bot.adapter.qqbot".into(),
            ConfigValue::from_json(&serde_json::json!({"enabled": true, "config": {}})),
        );
    first
        .config
        .apply(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigApplyRequest {
                candidate: ConfigValue::Object(candidate),
                expected_revision: snapshot.revision,
                dry_run: false,
            },
            ConfigContext::global(),
            &[capability::VALUE_WRITE.into(), capability::APPLY.into()],
        )
        .await
        .unwrap();
    drop(first);

    let error = match load_bootstrapped_product(&path).await {
        Ok(_) => panic!("reserved owner plugin unexpectedly loaded"),
        Err(error) => error,
    };
    assert!(error.contains("不得配置 owner 插件"));
}

#[tokio::test]
async fn enabling_local_workspace_takes_effect_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let path = bootstrap(root.path(), "");
    let first = load_bootstrapped_product(&path).await.unwrap();
    let snapshot = first
        .config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .unwrap();
    let ConfigValue::Object(mut candidate) = snapshot.value else {
        panic!("product config must be an object");
    };
    candidate.insert("workspace_enabled".into(), ConfigValue::Bool(true));
    first
        .config
        .apply(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigApplyRequest {
                candidate: ConfigValue::Object(candidate),
                expected_revision: snapshot.revision,
                dry_run: false,
            },
            ConfigContext::global(),
            &[capability::VALUE_WRITE.into(), capability::APPLY.into()],
        )
        .await
        .unwrap();
    drop(first);

    let restarted = load_bootstrapped_product(&path).await.unwrap();
    for id in ["mutsuki.agent.connections", "mutsuki.bot.router.flow"] {
        assert!(
            restarted
                .service
                .plugins
                .configured
                .iter()
                .any(|selection| selection.id == id && selection.enabled),
            "workspace component {id} was not enabled"
        );
    }
    assert_eq!(
        restarted.console.extensions,
        vec!["config", "qq", "agent", "bot-flow-editor"]
    );
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
