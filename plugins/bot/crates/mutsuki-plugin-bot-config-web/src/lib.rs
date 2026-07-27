//! Default Web configuration plugin.
//!
//! Registers typed RPC:
//! - config / providers.list
//! - config / schema.get
//! - config / snapshot.read
//! - config / validate
//! - config / apply
//! - config / metrics
//!
//! Frontend assets generate forms from ConfigDescriptor (Koishi-like shell + LiliaUI tokens).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_bot_config::{ConfigApplyRequest, ConfigContext, ConfigService, ConfigValue};
use mutsuki_web_extension::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, JsonValue, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};

pub const PLUGIN_ID: &str = "config";
pub const PLUGIN_VERSION: &str = "0.1.0";

/// Backend WebExtension that fronts a shared ConfigService.
pub struct ConfigWebExtension {
    service: Arc<ConfigService>,
    assets_root: Option<PathBuf>,
    capabilities: Vec<String>,
}

impl ConfigWebExtension {
    pub fn new(service: Arc<ConfigService>) -> Self {
        Self {
            service,
            assets_root: None,
            capabilities: vec![
                mutsuki_bot_config::capability::SCHEMA_READ.into(),
                mutsuki_bot_config::capability::VALUE_READ.into(),
                mutsuki_bot_config::capability::VALUE_WRITE.into(),
                mutsuki_bot_config::capability::SECRET_WRITE.into(),
                mutsuki_bot_config::capability::APPLY.into(),
                mutsuki_bot_config::capability::RELOAD.into(),
            ],
        }
    }

    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root = Some(root.into());
        self
    }

    fn block_on<F, T>(fut: F) -> Result<T, ExtensionError>
    where
        F: std::future::Future<Output = Result<T, mutsuki_bot_config::ConfigError>>,
    {
        // Sync RPC boundary: providers used here are in-memory / off Bot hot path.
        // Avoid nested tokio runtimes (axum may run current-thread).
        futures_executor::block_on(fut).map_err(map_config_error)
    }
}

impl WebExtension for ConfigWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        ExtensionManifest {
            manifest_version: EXTENSION_MANIFEST_VERSION,
            id: PLUGIN_ID.into(),
            version: PLUGIN_VERSION.into(),
            entry: "index.js".into(),
            capabilities: self.capabilities.clone(),
            permissions: vec!["pages".into(), "navigation".into()],
            assets: self
                .frontend_assets()
                .map(|assets| assets.manifest.assets)
                .unwrap_or_default(),
            protocol_version: WEB_PROTOCOL_VERSION.into(),
        }
    }

    fn frontend_assets(&self) -> Option<WebFrontendAssets> {
        let root = self.assets_root.as_ref()?;
        let manifest = load_or_synthesize_manifest(root).ok()?;
        Some(WebFrontendAssets {
            manifest,
            root_dir: root.clone(),
        })
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let service = self.service.clone();
        ctx.register("providers.list", {
            let service = service.clone();
            move |params| {
                let caps = caps_from_params(&params);
                let list = service.list_providers(&caps).map_err(map_config_error)?;
                Ok(serde_json::to_value(list).unwrap_or_default())
            }
        });

        ctx.register("schema.get", {
            let service = service.clone();
            move |params| {
                let caps = caps_from_params(&params);
                let provider_id = required_str(&params, "provider_id")?;
                let schema = service
                    .get_schema(&provider_id, &caps)
                    .map_err(map_config_error)?;
                Ok(serde_json::to_value(schema).unwrap_or_default())
            }
        });

        ctx.register("snapshot.read", {
            let service = service.clone();
            move |params| {
                let caps = caps_from_params(&params);
                let provider_id = required_str(&params, "provider_id")?;
                let context = context_from_params(&params)?;
                let snapshot =
                    ConfigWebExtension::block_on(service.read(&provider_id, context, &caps))?;
                Ok(serde_json::to_value(snapshot).unwrap_or_default())
            }
        });

        ctx.register("validate", {
            let service = service.clone();
            move |params| {
                let caps = caps_from_params(&params);
                let provider_id = required_str(&params, "provider_id")?;
                let context = context_from_params(&params)?;
                let candidate = candidate_from_params(&params)?;
                let result = ConfigWebExtension::block_on(service.validate(
                    &provider_id,
                    candidate,
                    context,
                    &caps,
                ))?;
                Ok(serde_json::to_value(result).unwrap_or_default())
            }
        });

        ctx.register("apply", {
            let service = service.clone();
            move |params| {
                let caps = caps_from_params(&params);
                let provider_id = required_str(&params, "provider_id")?;
                let context = context_from_params(&params)?;
                let request = apply_request_from_params(&params)?;
                let result = ConfigWebExtension::block_on(service.apply(
                    &provider_id,
                    request,
                    context,
                    &caps,
                ))?;
                Ok(serde_json::to_value(result).unwrap_or_default())
            }
        });

        ctx.register("metrics", {
            let service = service.clone();
            move |_params| Ok(serde_json::to_value(service.metrics_snapshot()).unwrap_or_default())
        });

        Ok(())
    }

    fn register_events(
        &self,
        ctx: &mut mutsuki_web_extension::EventRegistry,
    ) -> Result<(), ExtensionError> {
        ctx.register_topic("revision_changed");
        Ok(())
    }
}

fn map_config_error(err: mutsuki_bot_config::ConfigError) -> ExtensionError {
    ExtensionError::Registration(serde_json::to_string(&err).unwrap_or_else(|_| err.to_string()))
}

fn caps_from_params(params: &JsonValue) -> Vec<String> {
    params
        .get("capabilities")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_else(|| vec!["*".into()])
}

fn required_str(params: &JsonValue, key: &str) -> Result<String, ExtensionError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ExtensionError::Registration(format!("missing {key}")))
}

fn context_from_params(params: &JsonValue) -> Result<ConfigContext, ExtensionError> {
    let ctx_value = params.get("context").cloned().unwrap_or_else(
        || serde_json::json!({"scope": "plugin_instance", "plugin_instance_id": "default"}),
    );
    serde_json::from_value(ctx_value).map_err(|e| ExtensionError::Registration(e.to_string()))
}

fn candidate_from_params(params: &JsonValue) -> Result<ConfigValue, ExtensionError> {
    let raw = params
        .get("candidate")
        .cloned()
        .ok_or_else(|| ExtensionError::Registration("missing candidate".into()))?;
    config_value_from_json(raw)
}

fn apply_request_from_params(params: &JsonValue) -> Result<ConfigApplyRequest, ExtensionError> {
    let request_value = params
        .get("request")
        .cloned()
        .ok_or_else(|| ExtensionError::Registration("missing request".into()))?;
    let candidate = request_value
        .get("candidate")
        .cloned()
        .ok_or_else(|| ExtensionError::Registration("missing candidate".into()))?;
    let expected = request_value
        .get("expected_revision")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);
    let dry_run = request_value
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(ConfigApplyRequest {
        candidate: config_value_from_json(candidate)?,
        expected_revision: mutsuki_bot_config::ConfigRevision(expected),
        dry_run,
    })
}

fn config_value_from_json(raw: JsonValue) -> Result<ConfigValue, ExtensionError> {
    if raw.get("type").is_some() {
        serde_json::from_value(raw).map_err(|e| ExtensionError::Registration(e.to_string()))
    } else {
        Ok(ConfigValue::from_json(&raw))
    }
}

fn load_or_synthesize_manifest(root: &Path) -> Result<ExtensionManifest, ExtensionError> {
    let manifest_path = root.join("manifest.json");
    if manifest_path.exists() {
        let bytes =
            std::fs::read(&manifest_path).map_err(|e| ExtensionError::Manifest(e.to_string()))?;
        return serde_json::from_slice(&bytes).map_err(|e| ExtensionError::Manifest(e.to_string()));
    }
    let entry = root.join("index.js");
    let bytes = std::fs::read(&entry).map_err(|e| ExtensionError::Manifest(e.to_string()))?;
    Ok(ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![
            mutsuki_bot_config::capability::SCHEMA_READ.into(),
            mutsuki_bot_config::capability::VALUE_READ.into(),
            mutsuki_bot_config::capability::VALUE_WRITE.into(),
            mutsuki_bot_config::capability::SECRET_WRITE.into(),
            mutsuki_bot_config::capability::APPLY.into(),
        ],
        permissions: vec!["pages".into(), "navigation".into()],
        assets: vec![AssetEntry {
            path: "index.js".into(),
            content_hash: content_hash(&bytes),
            bytes: bytes.len() as u64,
        }],
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    })
}

/// Write bundled frontend assets for the default config web plugin.
pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    let js = include_str!("../assets/index.js");
    let bootstrap = include_str!("../assets/bootstrap.js");
    let css = include_str!("../assets/mutsuki-ui.css");
    let shell = include_str!("../assets/shell.html");
    std::fs::write(out_dir.join("index.js"), js)?;
    std::fs::write(out_dir.join("bootstrap.js"), bootstrap)?;
    std::fs::write(out_dir.join("mutsuki-ui.css"), css)?;
    std::fs::write(out_dir.join("shell.html"), shell)?;
    std::fs::write(out_dir.join("index.html"), shell)?;
    let entry_bytes = js.as_bytes();
    let bootstrap_bytes = bootstrap.as_bytes();
    let css_bytes = css.as_bytes();
    let shell_bytes = shell.as_bytes();
    let manifest = ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![
            mutsuki_bot_config::capability::SCHEMA_READ.into(),
            mutsuki_bot_config::capability::VALUE_READ.into(),
            mutsuki_bot_config::capability::VALUE_WRITE.into(),
            mutsuki_bot_config::capability::SECRET_WRITE.into(),
            mutsuki_bot_config::capability::APPLY.into(),
        ],
        permissions: vec!["pages".into(), "navigation".into()],
        assets: vec![
            AssetEntry {
                path: "index.js".into(),
                content_hash: content_hash(entry_bytes),
                bytes: entry_bytes.len() as u64,
            },
            AssetEntry {
                path: "bootstrap.js".into(),
                content_hash: content_hash(bootstrap_bytes),
                bytes: bootstrap_bytes.len() as u64,
            },
            AssetEntry {
                path: "mutsuki-ui.css".into(),
                content_hash: content_hash(css_bytes),
                bytes: css_bytes.len() as u64,
            },
            AssetEntry {
                path: "shell.html".into(),
                content_hash: content_hash(shell_bytes),
                bytes: shell_bytes.len() as u64,
            },
        ],
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    };
    std::fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest"),
    )?;
    Ok(out_dir.to_path_buf())
}
