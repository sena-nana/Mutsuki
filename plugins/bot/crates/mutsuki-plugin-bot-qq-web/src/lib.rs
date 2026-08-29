use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use mutsuki_bot_management::*;
use mutsuki_web_extension_api::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
    load_bundled_manifest,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "qq-bot";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_BOT_READ: &str = "bot.read";
pub const CAPABILITY_BOT_CONFIG_WRITE: &str = "bot.config.write";
pub const CAPABILITY_BOT_DELIVERY_WRITE: &str = "bot.delivery.write";
pub const CAPABILITY_BOT_INTERACTION_WRITE: &str = "bot.interaction.write";
pub const CAPABILITY_BOT_SECRET_STATUS: &str = "bot.secret.status";

pub const EXTRA_IMG_SRC: &[&str] = &[
    "https://*.qlogo.cn",
    "https://*.qpic.cn",
    "https://*.gtimg.cn",
    "https://*.qq.com.cn",
    "https://*.nt.qq.com.cn",
];

pub struct QqBotWebExtension {
    api: Arc<dyn QqBotManagementApi>,
    assets_root: Option<PathBuf>,
}

impl QqBotWebExtension {
    pub fn new(api: Arc<dyn QqBotManagementApi>) -> Self {
        Self {
            api,
            assets_root: None,
        }
    }

    #[must_use]
    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root = Some(root.into());
        self
    }
}

impl WebExtension for QqBotWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        manifest(
            self.frontend_assets()
                .map(|assets| assets.manifest.assets)
                .unwrap_or_default(),
        )
    }

    fn frontend_assets(&self) -> Option<WebFrontendAssets> {
        let root = self.assets_root.as_ref()?;
        Some(WebFrontendAssets {
            manifest: load_bundled_manifest(root, manifest).ok()?,
            root_dir: root.clone(),
        })
    }

    fn register_rpc(&self, registry: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let api = self.api.clone();
        registry.register_async_contextual("snapshot", move |context, params| {
            let api = api.clone();
            async move {
                context.require(CAPABILITY_BOT_READ)?;
                let include_secret_status = context.capabilities().iter().any(|capability| {
                    capability == "*" || capability == CAPABILITY_BOT_SECRET_STATUS
                });
                let query = params.get("query").and_then(Value::as_str).unwrap_or("");
                let mut snapshot = api
                    .snapshot(query, include_secret_status)
                    .await
                    .map_err(domain_error)?;
                if !include_secret_status {
                    for account in &mut snapshot.accounts {
                        account.credential_reference.clear();
                        account.credential_status = "restricted".into();
                    }
                }
                serde_json::to_value(snapshot).map_err(|error| {
                    ExtensionError::Registration(format!("snapshot encode failed: {error}"))
                })
            }
        });

        let api = self.api.clone();
        registry.register_async_contextual("write", move |context, params| {
            let api = api.clone();
            async move {
                let mut request: QqManagementWriteRequest = serde_json::from_value(
                    params
                        .get("request")
                        .cloned()
                        .ok_or_else(|| ExtensionError::Registration("missing request".into()))?,
                )
                .map_err(|error| ExtensionError::Registration(error.to_string()))?;
                if request.operation_id.trim().is_empty() {
                    request.operation_id = uuid::Uuid::new_v4().to_string();
                }
                let confirmed = params
                    .get("confirmed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if requires_confirmation(&request.action) && !confirmed {
                    return Err(ExtensionError::Registration(
                        "dangerous action requires confirmation".into(),
                    ));
                }
                context.require(required_capability(&request.action))?;
                serde_json::to_value(
                    api.write(context.principal_id(), request)
                        .await
                        .map_err(domain_error)?,
                )
                .map_err(|error| {
                    ExtensionError::Registration(format!("write result encode failed: {error}"))
                })
            }
        });

        let api = self.api.clone();
        registry.register_async_contextual("deliveries.list", move |context, params| {
            let api = api.clone();
            async move {
                context.require(CAPABILITY_BOT_READ)?;
                let page = api
                    .delivery_page(
                        params.get("query").and_then(Value::as_str).unwrap_or(""),
                        params.get("after").and_then(Value::as_str),
                        page_limit(&params),
                    )
                    .await
                    .map_err(domain_error)?;
                serde_json::to_value(page).map_err(|error| {
                    ExtensionError::Registration(format!("delivery page encode failed: {error}"))
                })
            }
        });

        let api = self.api.clone();
        registry.register_async_contextual("interactions.list", move |context, params| {
            let api = api.clone();
            async move {
                context.require(CAPABILITY_BOT_READ)?;
                let page = api
                    .interaction_page(
                        params.get("query").and_then(Value::as_str).unwrap_or(""),
                        params.get("after").and_then(Value::as_str),
                        page_limit(&params),
                    )
                    .await
                    .map_err(domain_error)?;
                serde_json::to_value(page).map_err(|error| {
                    ExtensionError::Registration(format!("interaction page encode failed: {error}"))
                })
            }
        });
        Ok(())
    }

    fn register_events(
        &self,
        registry: &mut mutsuki_web_extension_api::EventRegistry,
    ) -> Result<(), ExtensionError> {
        registry.register_topic("changed");
        Ok(())
    }

    fn extra_img_src(&self) -> Vec<String> {
        EXTRA_IMG_SRC
            .iter()
            .map(|source| (*source).to_string())
            .collect()
    }
}

/// Writes the embedded QQ management frontend and its content-addressed manifest.
///
/// # Errors
///
/// Returns an I/O error when directory creation, manifest encoding, or asset writes fail.
pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    let js = include_str!("../assets/index.js");
    std::fs::write(out_dir.join("index.js"), js)?;
    let assets = vec![AssetEntry {
        path: "index.js".into(),
        content_hash: content_hash(js.as_bytes()),
        bytes: js.len() as u64,
    }];
    let manifest = serde_json::to_vec_pretty(&manifest(assets)).map_err(std::io::Error::other)?;
    std::fs::write(out_dir.join("manifest.json"), manifest)?;
    Ok(out_dir.to_path_buf())
}

fn manifest(assets: Vec<AssetEntry>) -> ExtensionManifest {
    ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![
            CAPABILITY_BOT_READ.into(),
            CAPABILITY_BOT_CONFIG_WRITE.into(),
            CAPABILITY_BOT_DELIVERY_WRITE.into(),
            CAPABILITY_BOT_INTERACTION_WRITE.into(),
            CAPABILITY_BOT_SECRET_STATUS.into(),
        ],
        permissions: vec!["pages".into(), "navigation".into()],
        assets,
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    }
}

fn domain_error(QqManagementError { code, message }: QqManagementError) -> ExtensionError {
    ExtensionError::Registration(json!({"code": code, "message": message}).to_string())
}

fn page_limit(params: &Value) -> u32 {
    params
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|limit| u32::try_from(limit).ok())
        .unwrap_or(50)
        .clamp(1, 100)
}

fn required_capability(action: &QqManagementAction) -> &'static str {
    match action {
        QqManagementAction::AccountSetEnabled { .. }
        | QqManagementAction::AccountHealthCheck { .. }
        | QqManagementAction::AccountReconnect { .. }
        | QqManagementAction::AccountSendTest { .. } => CAPABILITY_BOT_CONFIG_WRITE,
        QqManagementAction::InteractionCancel { .. } => CAPABILITY_BOT_INTERACTION_WRITE,
        QqManagementAction::DeliveryRetry { .. }
        | QqManagementAction::DeliveryCancel { .. }
        | QqManagementAction::DeliveryPreview { .. } => CAPABILITY_BOT_DELIVERY_WRITE,
    }
}

fn requires_confirmation(action: &QqManagementAction) -> bool {
    !matches!(action, QqManagementAction::DeliveryPreview { .. })
}
