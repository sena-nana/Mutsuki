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

use mutsuki_config_service::{ConfigApplyRequest, ConfigContext, ConfigService, ConfigValue};
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
                mutsuki_config_service::capability::SCHEMA_READ.into(),
                mutsuki_config_service::capability::VALUE_READ.into(),
                mutsuki_config_service::capability::VALUE_WRITE.into(),
                mutsuki_config_service::capability::SECRET_WRITE.into(),
                mutsuki_config_service::capability::APPLY.into(),
                mutsuki_config_service::capability::RELOAD.into(),
            ],
        }
    }

    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root = Some(root.into());
        self
    }

    fn block_on<F, T>(fut: F) -> Result<T, ExtensionError>
    where
        F: std::future::Future<Output = Result<T, mutsuki_config_service::ConfigError>>,
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
        ctx.register_contextual("providers.list", {
            let service = service.clone();
            move |context, _params| {
                context.require(mutsuki_config_service::capability::SCHEMA_READ)?;
                let list = service
                    .list_providers(context.capabilities())
                    .map_err(map_config_error)?;
                Ok(serde_json::to_value(list).unwrap_or_default())
            }
        });

        ctx.register_contextual("schema.get", {
            let service = service.clone();
            move |context, params| {
                context.require(mutsuki_config_service::capability::SCHEMA_READ)?;
                let provider_id = required_str(&params, "provider_id")?;
                let schema = service
                    .get_schema(&provider_id, context.capabilities())
                    .map_err(map_config_error)?;
                Ok(serde_json::to_value(schema).unwrap_or_default())
            }
        });

        ctx.register_contextual("snapshot.read", {
            let service = service.clone();
            move |call_context, params| {
                call_context.require(mutsuki_config_service::capability::VALUE_READ)?;
                let provider_id = required_str(&params, "provider_id")?;
                let context = context_from_params(&params)?;
                let snapshot = ConfigWebExtension::block_on(service.read(
                    &provider_id,
                    context,
                    call_context.capabilities(),
                ))?;
                Ok(serde_json::to_value(snapshot).unwrap_or_default())
            }
        });

        ctx.register_contextual("validate", {
            let service = service.clone();
            move |call_context, params| {
                call_context.require(mutsuki_config_service::capability::VALUE_WRITE)?;
                let provider_id = required_str(&params, "provider_id")?;
                let context = context_from_params(&params)?;
                let candidate = candidate_from_params(&params)?;
                let result = ConfigWebExtension::block_on(service.validate(
                    &provider_id,
                    candidate,
                    context,
                    call_context.capabilities(),
                ))?;
                Ok(serde_json::to_value(result).unwrap_or_default())
            }
        });

        ctx.register_contextual("apply", {
            let service = service.clone();
            move |call_context, params| {
                call_context.require(mutsuki_config_service::capability::APPLY)?;
                let provider_id = required_str(&params, "provider_id")?;
                let context = context_from_params(&params)?;
                let request = apply_request_from_params(&params)?;
                let result = ConfigWebExtension::block_on(service.apply(
                    &provider_id,
                    request,
                    context,
                    call_context.capabilities(),
                ))?;
                Ok(serde_json::to_value(result).unwrap_or_default())
            }
        });

        ctx.register_contextual("metrics", {
            let service = service.clone();
            move |context, _params| {
                context.require(mutsuki_config_service::capability::SCHEMA_READ)?;
                Ok(serde_json::to_value(service.metrics_snapshot()).unwrap_or_default())
            }
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

fn map_config_error(err: mutsuki_config_service::ConfigError) -> ExtensionError {
    ExtensionError::Registration(serde_json::to_string(&err).unwrap_or_else(|_| err.to_string()))
}

fn required_str(params: &JsonValue, key: &str) -> Result<String, ExtensionError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ExtensionError::Registration(format!("missing {key}")))
}

fn context_from_params(params: &JsonValue) -> Result<ConfigContext, ExtensionError> {
    params.get("context").cloned().map_or_else(
        || Ok(ConfigContext::global()),
        |value| {
            serde_json::from_value(value)
                .map_err(|error| ExtensionError::Registration(error.to_string()))
        },
    )
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
        .unwrap_or(0);
    let dry_run = request_value
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(ConfigApplyRequest {
        candidate: config_value_from_json(candidate)?,
        expected_revision: mutsuki_config_service::ConfigRevision(expected),
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
            mutsuki_config_service::capability::SCHEMA_READ.into(),
            mutsuki_config_service::capability::VALUE_READ.into(),
            mutsuki_config_service::capability::VALUE_WRITE.into(),
            mutsuki_config_service::capability::SECRET_WRITE.into(),
            mutsuki_config_service::capability::APPLY.into(),
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
    let css = include_str!("../assets/mutsuki-ui.css");
    std::fs::write(out_dir.join("index.js"), js)?;
    std::fs::write(out_dir.join("mutsuki-ui.css"), css)?;
    let entry_bytes = js.as_bytes();
    let css_bytes = css.as_bytes();
    let manifest = ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![
            mutsuki_config_service::capability::SCHEMA_READ.into(),
            mutsuki_config_service::capability::VALUE_READ.into(),
            mutsuki_config_service::capability::VALUE_WRITE.into(),
            mutsuki_config_service::capability::SECRET_WRITE.into(),
            mutsuki_config_service::capability::APPLY.into(),
        ],
        permissions: vec!["pages".into(), "navigation".into()],
        assets: vec![
            AssetEntry {
                path: "index.js".into(),
                content_hash: content_hash(entry_bytes),
                bytes: entry_bytes.len() as u64,
            },
            AssetEntry {
                path: "mutsuki-ui.css".into(),
                content_hash: content_hash(css_bytes),
                bytes: css_bytes.len() as u64,
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
