use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_agent_service_host_integration::AgentConnectionRegistry;
use mutsuki_config_service::{
    ConfigApplyMode, ConfigConstraints, ConfigContext, ConfigDescriptor, ConfigKey,
    ConfigMutability, ConfigNode, ConfigPresentation, ConfigProviderId, ConfigScope, ConfigService,
    ConfigValue, ConfigValueType, LocalizedText, MemoryConfigProvider, RestartPolicy, capability,
};
use mutsuki_plugin_config_sqlite::{
    PLUGIN_ID as SQLITE_REPOSITORY_PLUGIN_ID, SqliteConfigRepository,
};
use mutsuki_service_config::{ServiceConfig, recover_host_secret_transaction};
use serde::Deserialize;

use crate::{
    PRODUCT_CONFIG_PROVIDER_ID, ProductConfigOptions, configured_product_owner_selections,
    configured_product_selections, product_config_service_with_options, product_seed_defaults,
    register_configured_product_providers,
};

pub const SERVICE_CONFIG_PROVIDER_ID: &str = "mutsuki.service.runtime";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotBootstrap {
    pub host: BootstrapHost,
    #[serde(default)]
    pub security: BootstrapSecurity,
    #[serde(default)]
    pub plugin_discovery: BootstrapPluginDiscovery,
    pub config_repository: ConfigRepositoryBootstrap,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapHost {
    pub instance_id: String,
    pub home_dir: PathBuf,
    pub data_dir: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BootstrapSecurity {
    pub secret_file: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BootstrapPluginDiscovery {
    pub dynamic_dirs: Vec<PathBuf>,
    pub disabled_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRepositoryBootstrap {
    pub repository_plugin_id: String,
    pub document_namespace: String,
    #[serde(default)]
    pub options: BTreeMap<String, toml::Value>,
}

pub struct BootstrappedProduct {
    pub service: ServiceConfig,
    pub config: Arc<ConfigService>,
    pub console: LocalConsoleConfig,
    pub root: PathBuf,
    pub agent_connections: AgentConnectionRegistry,
}

#[derive(Clone, Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalConsoleConfig {
    pub enabled: bool,
    pub listen: String,
    pub auth_token_key: Option<String>,
    pub extensions: Vec<String>,
    pub release_set: Option<String>,
}

pub async fn load_bootstrapped_product(path: &Path) -> Result<BootstrappedProduct, String> {
    let bootstrap_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    let path = bootstrap_path.as_path();
    let content = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read bootstrap {}: {error}", path.display()))?;
    let bootstrap: BotBootstrap = toml::from_str(&content)
        .map_err(|error| format!("invalid bootstrap {}: {error}", path.display()))?;
    if bootstrap.config_repository.repository_plugin_id != SQLITE_REPOSITORY_PLUGIN_ID {
        return Err(format!(
            "unknown bootstrap repository plugin `{}`",
            bootstrap.config_repository.repository_plugin_id
        ));
    }
    let root = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    if let Some(secret_file) = bootstrap.security.secret_file.as_deref() {
        recover_host_secret_transaction(&resolve(&root, secret_file))
            .map_err(|error| error.to_string())?;
    }
    let repository_path = bootstrap
        .config_repository
        .options
        .get("path")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "SQLite repository options.path is required".to_string())?;
    let repository_path = resolve(&root, Path::new(repository_path));
    if let Some(parent) = repository_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create repository directory: {error}"))?;
    }
    let repository = Arc::new(
        SqliteConfigRepository::open(
            repository_path,
            &bootstrap.config_repository.document_namespace,
        )
        .map_err(|error| error.to_string())?,
    );
    let config = product_config_service_with_options(ProductConfigOptions::new(repository))
        .map_err(|error| error.to_string())?;
    config
        .create_if_absent(
            PRODUCT_CONFIG_PROVIDER_ID,
            product_seed_defaults(),
            ConfigContext::global(),
        )
        .await
        .map_err(|error| error.to_string())?;
    let seed = service_seed(&bootstrap, &root);
    config
        .registry()
        .register(Arc::new(MemoryConfigProvider::new(
            service_descriptor(),
            ConfigValue::from_json(
                &serde_json::to_value(&seed).map_err(|error| error.to_string())?,
            ),
            ConfigApplyMode::RequireRestart,
        )))
        .map_err(|error| error.to_string())?;
    config
        .create_if_absent(
            SERVICE_CONFIG_PROVIDER_ID,
            ConfigValue::from_json(
                &serde_json::to_value(&seed).map_err(|error| error.to_string())?,
            ),
            ConfigContext::global(),
        )
        .await
        .map_err(|error| error.to_string())?;

    let snapshot = config
        .read(
            SERVICE_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .map_err(|error| error.to_string())?;
    let mut service: ServiceConfig = serde_json::from_value(snapshot.value.to_json())
        .map_err(|error| format!("stored ServiceConfig is invalid: {error}"))?;
    apply_bootstrap_boundaries(&mut service, &bootstrap, &root);

    let product_snapshot = config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .map_err(|error| error.to_string())?;
    if product_snapshot.schema_version != 3 || product_snapshot.value_version != 3 {
        return Err(format!(
            "product.config.version_unsupported: stored mutsuki.product schema/value version {}/{} is unsupported; recreate the product config repository for version 3",
            product_snapshot.schema_version, product_snapshot.value_version
        ));
    }
    let product = product_snapshot.value.to_json();
    let console = decode_console(&product)?;
    ensure_local_auth_secret(&service, &console, path)?;
    let mut service = service
        .finalize_bootstrap(path, None)
        .map_err(|error| error.to_string())?;
    register_configured_product_providers(&config, service.host_secret_store())
        .await
        .map_err(|error| error.to_string())?;
    let owner_selections = configured_product_owner_selections(&config)
        .await
        .map_err(|error| error.to_string())?;
    service.plugins.configured = configured_product_selections(&product, owner_selections)
        .map_err(|error| error.to_string())?;
    let agent_connections = AgentConnectionRegistry::new();
    Ok(BootstrappedProduct {
        service,
        config,
        console,
        root,
        agent_connections,
    })
}

fn service_seed(bootstrap: &BotBootstrap, root: &Path) -> ServiceConfig {
    let mut service = ServiceConfig::default();
    service.service.profile = "bot".into();
    service.plugins.configured.clear();
    apply_bootstrap_boundaries(&mut service, bootstrap, root);
    service
}

fn apply_bootstrap_boundaries(service: &mut ServiceConfig, bootstrap: &BotBootstrap, root: &Path) {
    // This local product is administered through the loopback Web Console; it does not expose
    // the optional Unix-domain control socket.
    service.ipc.enabled = false;
    service
        .service
        .instance_id
        .clone_from(&bootstrap.host.instance_id);
    service.service.home_dir = resolve(root, &bootstrap.host.home_dir);
    service.service.data_dir = resolve(root, &bootstrap.host.data_dir);
    service.security.secret_file = bootstrap
        .security
        .secret_file
        .as_deref()
        .map(|path| resolve(root, path));
    service.plugins.dynamic_dirs = bootstrap
        .plugin_discovery
        .dynamic_dirs
        .iter()
        .map(|path| resolve(root, path))
        .collect();
    if let Some(path) = &bootstrap.plugin_discovery.disabled_dir {
        service.plugins.disabled_dir = resolve(root, path);
    }
}

fn decode_console(product: &serde_json::Value) -> Result<LocalConsoleConfig, String> {
    let mut extensions = vec!["config".to_string()];
    if product
        .get("workspace_enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        extensions.extend(["qq".into(), "agent".into(), "bot-flow-editor".into()]);
    }
    serde_json::from_value(serde_json::json!({
        "enabled": product.get("console_enabled").and_then(serde_json::Value::as_bool).unwrap_or(true),
        "listen": product.get("console_listen").and_then(serde_json::Value::as_str).unwrap_or("127.0.0.1:0"),
        "auth_token_key": product.get("auth_token_key").and_then(serde_json::Value::as_str),
        "extensions": extensions,
        "release_set": serde_json::Value::Null,
    }))
    .map_err(|error| error.to_string())
}

fn ensure_local_auth_secret(
    service: &ServiceConfig,
    console: &LocalConsoleConfig,
    bootstrap_path: &Path,
) -> Result<(), String> {
    if !console.enabled {
        return Ok(());
    }
    let key = console
        .auth_token_key
        .as_deref()
        .ok_or_else(|| "enabled console requires auth_token_key".to_string())?;
    let configured = service
        .security
        .secret_file
        .as_ref()
        .ok_or_else(|| "enabled console requires bootstrap security.secret_file".to_string())?;
    let secret_path = resolve(
        bootstrap_path.parent().unwrap_or_else(|| Path::new(".")),
        configured,
    );
    let mut document = if secret_path.exists() {
        let content = std::fs::read_to_string(&secret_path).map_err(|error| error.to_string())?;
        toml::from_str::<toml::Value>(&content).map_err(|error| error.to_string())?
    } else {
        toml::Value::Table(Default::default())
    };
    let root = document
        .as_table_mut()
        .ok_or_else(|| "local secret file must be a TOML table".to_string())?;
    let secrets = root
        .entry("secrets")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| "local secret file [secrets] must be a TOML table".to_string())?;
    if secrets
        .get(key)
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        ensure_private_permissions(&secret_path)?;
        return Ok(());
    }
    if let Some(parent) = secret_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    secrets.insert(key.into(), toml::Value::String(token));
    write_private_toml(&secret_path, &document)
}

fn write_private_toml(path: &Path, document: &toml::Value) -> Result<(), String> {
    let content = toml::to_string_pretty(document).map_err(|error| error.to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("local.secret.toml");
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        use std::io::Write as _;
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(content.as_bytes())
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        ensure_private_permissions(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn ensure_private_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn service_descriptor() -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new(SERVICE_CONFIG_PROVIDER_ID),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new("Service Runtime"),
        description: None,
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: ConfigKey::new("service"),
            value_type: ConfigValueType::Map {
                key_strategy: mutsuki_config_service::MapKeyStrategy::FreeString,
                value: Box::new(ConfigValueType::Object),
            },
            title: LocalizedText::new("Service Runtime"),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::HostRestart,
            children: Vec::new(),
        },
        groups: Vec::new(),
    }
}

fn resolve(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TargetedPluginReloadLifecycle;
    use mutsuki_agent_service_host_integration::{
        AGENT_CONNECTION_MANAGEMENT_SERVICE_ID, AGENT_CONNECTIONS_PLUGIN_ID,
        AgentConnectionManager, LOCAL_AGENT_API_KEY, LOCAL_AGENT_API_KEY_FIELD,
        LOCAL_AGENT_CONFIG_PROVIDER_ID, LOCAL_AGENT_PLUGIN_ID, LocalAgentConfig,
        local_agent_config_value,
    };
    use mutsuki_bot_flow::BotFlowRegistry;
    use mutsuki_config_service::{ConfigApplyRequest, SecretState, SecretValue};
    use mutsuki_plugin_bot_adapter_qqbot::QQBOT_ADAPTER_PLUGIN_ID;
    use mutsuki_plugin_bot_event_router::BOT_FLOW_REGISTRY_SERVICE_ID;

    fn enable_runtime_test_services(product: &mut BootstrappedProduct) {
        for id in [AGENT_CONNECTIONS_PLUGIN_ID, "mutsuki.bot.router.flow"] {
            product
                .service
                .plugins
                .configured
                .iter_mut()
                .find(|selection| selection.id == id)
                .expect("test runtime selection")
                .enabled = true;
        }
    }

    #[tokio::test]
    async fn local_bootstrap_declares_product_components_and_private_console_secret() {
        let root = tempfile::tempdir().unwrap();
        let bootstrap_path = root.path().join("local.toml");
        std::fs::write(
            &bootstrap_path,
            r#"
[host]
instance_id = "test-bot"
home_dir = "home"
data_dir = "home/data"

[security]
secret_file = "local.secret.toml"

[config_repository]
repository_plugin_id = "mutsuki.config.repository.sqlite"
document_namespace = "test-bot"

[config_repository.options]
path = "home/config.sqlite3"
"#,
        )
        .unwrap();

        let product = load_bootstrapped_product(&bootstrap_path).await.unwrap();
        for id in [
            AGENT_CONNECTIONS_PLUGIN_ID,
            "mutsuki.bot.router.flow",
            QQBOT_ADAPTER_PLUGIN_ID,
            LOCAL_AGENT_PLUGIN_ID,
            "mutsuki.plugin.bot.agent",
        ] {
            assert!(
                product
                    .service
                    .plugins
                    .configured
                    .iter()
                    .any(|selection| selection.id == id && !selection.enabled),
                "missing disabled product selection {id}: {:?}",
                product.service.plugins.configured
            );
        }
        assert!(!product.service.ipc.enabled);
        assert_eq!(product.console.listen, "127.0.0.1:8787");
        assert_eq!(product.console.extensions, vec!["config"]);
        let secret_path = root.path().join("local.secret.toml");
        let content = std::fs::read_to_string(&secret_path).unwrap();
        assert!(!content.contains("mutsuki.web.console.token ="));
        let parsed: toml::Value = toml::from_str(&content).unwrap();
        assert!(
            parsed["secrets"]["mutsuki.web.console.token"]
                .as_str()
                .is_some_and(|token| token.len() == 64)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(secret_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn owner_config_apply_persists_secret_and_preserves_unrelated_services() {
        let root = tempfile::tempdir().unwrap();
        let bootstrap_path = root.path().join("local.toml");
        std::fs::write(
            &bootstrap_path,
            r#"
[host]
instance_id = "test-bot"
home_dir = "home"
data_dir = "home/data"

[security]
secret_file = "local.secret.toml"

[config_repository]
repository_plugin_id = "mutsuki.config.repository.sqlite"
document_namespace = "test-bot"

[config_repository.options]
path = "home/config.sqlite3"
"#,
        )
        .unwrap();

        let mut product = load_bootstrapped_product(&bootstrap_path).await.unwrap();
        enable_runtime_test_services(&mut product);
        let runtime = crate::assemble_service_with_connections(
            product.service.clone(),
            product.config.clone(),
            product.agent_connections.clone(),
        )
        .unwrap()
        .start()
        .await
        .unwrap();
        product
            .config
            .set_lifecycle(Arc::new(TargetedPluginReloadLifecycle::new(
                runtime.handle(),
            )));
        let flow_before = runtime
            .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
            .unwrap();
        let connections_before = runtime
            .host_service::<AgentConnectionManager>(AGENT_CONNECTION_MANAGEMENT_SERVICE_ID)
            .unwrap();

        let snapshot = product
            .config
            .read(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap();
        let mut local = LocalAgentConfig::default();
        local.endpoint = "http://127.0.0.1:43111/v1".into();
        local.model = "fixture-model".into();
        let mut candidate = local_agent_config_value(false, &local);
        candidate.as_object_mut().unwrap().insert(
            LOCAL_AGENT_API_KEY_FIELD.into(),
            ConfigValue::Secret(SecretState::Set {
                value: SecretValue::new("fixture-api-key"),
            }),
        );
        let applied = product
            .config
            .apply(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigApplyRequest {
                    candidate,
                    expected_revision: snapshot.revision,
                    dry_run: false,
                },
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap();
        assert!(applied.applied);
        assert_eq!(
            product
                .service
                .host_secret_store()
                .resolve(LOCAL_AGENT_API_KEY)
                .as_deref(),
            Some("fixture-api-key")
        );
        let stored = product
            .config
            .read(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap();
        assert!(!format!("{stored:?}").contains("fixture-api-key"));
        let flow_after = runtime
            .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
            .unwrap();
        let connections_after = runtime
            .host_service::<AgentConnectionManager>(AGENT_CONNECTION_MANAGEMENT_SERVICE_ID)
            .unwrap();
        assert!(Arc::ptr_eq(&flow_before, &flow_after));
        assert!(Arc::ptr_eq(&connections_before, &connections_after));
        runtime.shutdown().await;
        drop(product);

        let restored = load_bootstrapped_product(&bootstrap_path).await.unwrap();
        let local = restored
            .service
            .plugins
            .configured
            .iter()
            .find(|selection| selection.id == LOCAL_AGENT_PLUGIN_ID)
            .unwrap();
        assert_eq!(local.config["endpoint"], "http://127.0.0.1:43111/v1");
        assert_eq!(
            restored.service.secret(LOCAL_AGENT_API_KEY).as_deref(),
            Some("fixture-api-key")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_agent_preflight_restores_secret_document_and_runtime_generation() {
        let root = tempfile::tempdir().unwrap();
        let bootstrap_path = root.path().join("local.toml");
        std::fs::write(
            &bootstrap_path,
            r#"
[host]
instance_id = "test-bot"
home_dir = "home"
data_dir = "home/data"

[security]
secret_file = "local.secret.toml"

[config_repository]
repository_plugin_id = "mutsuki.config.repository.sqlite"
document_namespace = "test-bot"

[config_repository.options]
path = "home/config.sqlite3"
"#,
        )
        .unwrap();
        let mut product = load_bootstrapped_product(&bootstrap_path).await.unwrap();
        enable_runtime_test_services(&mut product);
        let runtime = crate::assemble_service_with_connections(
            product.service.clone(),
            product.config.clone(),
            product.agent_connections.clone(),
        )
        .unwrap()
        .start()
        .await
        .unwrap();
        product
            .config
            .set_lifecycle(Arc::new(TargetedPluginReloadLifecycle::new(
                runtime.handle(),
            )));
        let flow_before = runtime
            .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
            .unwrap();
        let snapshot = product
            .config
            .read(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap();
        let mut local = LocalAgentConfig::default();
        local.endpoint = "http://127.0.0.1:9/v1".into();
        local.model = "unreachable-model".into();
        let mut candidate = local_agent_config_value(true, &local);
        candidate.as_object_mut().unwrap().insert(
            LOCAL_AGENT_API_KEY_FIELD.into(),
            ConfigValue::Secret(SecretState::Set {
                value: SecretValue::new("must-roll-back"),
            }),
        );
        let error = product
            .config
            .apply(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigApplyRequest {
                    candidate,
                    expected_revision: snapshot.revision,
                    dry_run: false,
                },
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("must-roll-back"));
        assert!(
            product
                .service
                .host_secret_store()
                .resolve(LOCAL_AGENT_API_KEY)
                .is_none()
        );
        assert!(
            !std::fs::read_to_string(root.path().join("local.secret.toml"))
                .unwrap()
                .contains("must-roll-back")
        );
        let after = product
            .config
            .read(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap();
        assert_eq!(after.revision, snapshot.revision);
        assert_eq!(after.value.to_json()["enabled"], false);
        let flow_after = runtime
            .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
            .unwrap();
        assert!(Arc::ptr_eq(&flow_before, &flow_after));
        runtime.shutdown().await;
    }
}
