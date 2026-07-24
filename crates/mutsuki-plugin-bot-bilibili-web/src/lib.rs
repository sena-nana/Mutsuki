//! Bilibili console WebExtension: login state + subscription management RPC.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_bot_protocol::BotTarget;
use mutsuki_plugin_bot_bilibili::{BilibiliManagementService, BilibiliPollKind};
use mutsuki_plugin_bot_control_web::{CAPABILITY_RUNTIME_READ, CAPABILITY_RUNTIME_WRITE};
use mutsuki_web_extension::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde_json::{Value as JsonValue, json};

pub const PLUGIN_ID: &str = "bilibili";
pub const PLUGIN_VERSION: &str = "0.1.0";

/// Fixed actor id used for console-initiated QR sessions.
pub const CONSOLE_LOGIN_ACTOR: &str = "web-console";

pub struct BilibiliWebExtension {
    service: Arc<BilibiliManagementService>,
    assets_root: Option<PathBuf>,
}

impl BilibiliWebExtension {
    pub fn new(service: Arc<BilibiliManagementService>) -> Self {
        Self {
            service,
            assets_root: None,
        }
    }

    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root = Some(root.into());
        self
    }
}

impl WebExtension for BilibiliWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        ExtensionManifest {
            manifest_version: EXTENSION_MANIFEST_VERSION,
            id: PLUGIN_ID.into(),
            version: PLUGIN_VERSION.into(),
            entry: "index.js".into(),
            capabilities: vec![
                CAPABILITY_RUNTIME_READ.into(),
                CAPABILITY_RUNTIME_WRITE.into(),
            ],
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
        Some(WebFrontendAssets {
            manifest: load_or_synthesize_manifest(root).ok()?,
            root_dir: root.clone(),
        })
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let service = self.service.clone();

        ctx.register("status", {
            let service = service.clone();
            move |params| {
                require_runtime_read(&params)?;
                Ok(serde_json::to_value(service.status()).unwrap_or_default())
            }
        });

        ctx.register("login.start", {
            let service = service.clone();
            move |params| {
                require_runtime_write(&params)?;
                let actor = optional_str(&params, "actor_id").unwrap_or(CONSOLE_LOGIN_ACTOR.into());
                let result = service.login_start(&actor).map_err(map_bili_error)?;
                Ok(serde_json::to_value(result).unwrap_or_default())
            }
        });

        ctx.register("login.poll", {
            let service = service.clone();
            move |params| {
                require_runtime_read(&params)?;
                let actor = optional_str(&params, "actor_id").unwrap_or(CONSOLE_LOGIN_ACTOR.into());
                let result = service.login_poll(&actor).map_err(map_bili_error)?;
                Ok(serde_json::to_value(result).unwrap_or_default())
            }
        });

        ctx.register("credential.clear", {
            let service = service.clone();
            move |params| {
                require_runtime_write(&params)?;
                service.credential_clear().map_err(map_bili_error)?;
                Ok(json!({ "ok": true }))
            }
        });

        ctx.register("subscriptions.list", {
            let service = service.clone();
            move |params| {
                require_runtime_read(&params)?;
                let actor = optional_str(&params, "operator_user_id").unwrap_or_default();
                let is_admin = params
                    .get("is_admin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let list = service.list(&actor, is_admin).map_err(map_bili_error)?;
                Ok(json!({ "subscriptions": list }))
            }
        });

        ctx.register("subscriptions.subscribe", {
            let service = service.clone();
            move |params| {
                require_runtime_write(&params)?;
                let subscription_id = required_str(&params, "subscription_id")?;
                let uid = required_u64(&params, "uid")?;
                let notifications = parse_notifications_json(&params)?;
                let target = parse_target(&params)?;
                let outbound_binding = required_str(&params, "outbound_binding")?;
                let view = service
                    .subscribe(
                        subscription_id,
                        uid,
                        notifications,
                        target,
                        outbound_binding,
                    )
                    .map_err(map_bili_error)?;
                Ok(serde_json::to_value(view).unwrap_or_default())
            }
        });

        ctx.register("subscriptions.unsubscribe", {
            let service = service.clone();
            move |params| {
                require_runtime_write(&params)?;
                let subscription_id = required_str(&params, "subscription_id")?;
                service
                    .unsubscribe(&subscription_id)
                    .map_err(map_bili_error)?;
                Ok(json!({ "ok": true }))
            }
        });

        ctx.register("subscriptions.set_paused", {
            let service = service.clone();
            move |params| {
                require_runtime_write(&params)?;
                let actor = optional_str(&params, "operator_user_id").unwrap_or_default();
                let is_admin = params
                    .get("is_admin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let selector = optional_str(&params, "selector");
                let paused = params
                    .get("paused")
                    .and_then(|v| v.as_bool())
                    .ok_or_else(|| ExtensionError::Registration("missing paused".into()))?;
                let view = service
                    .set_paused(&actor, is_admin, selector.as_deref(), paused)
                    .map_err(map_bili_error)?;
                Ok(serde_json::to_value(view).unwrap_or_default())
            }
        });

        ctx.register("subscriptions.preview", {
            let service = service.clone();
            move |params| {
                require_runtime_read(&params)?;
                let actor = optional_str(&params, "operator_user_id").unwrap_or_default();
                let is_admin = params
                    .get("is_admin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let selector = optional_str(&params, "selector");
                let card = service
                    .preview(&actor, is_admin, selector.as_deref())
                    .map_err(map_bili_error)?;
                Ok(serde_json::to_value(card).unwrap_or_default())
            }
        });

        ctx.register("binding.start", {
            let service = service.clone();
            move |params| {
                require_runtime_write(&params)?;
                let operator = required_str(&params, "operator_user_id")?;
                let uid = required_u64(&params, "uid")?;
                let seed = optional_str(&params, "challenge_seed")
                    .unwrap_or_else(|| format!("web-{}", operator));
                let result = service
                    .bind_start(&operator, uid, &seed)
                    .map_err(map_bili_error)?;
                Ok(serde_json::to_value(result).unwrap_or_default())
            }
        });

        ctx.register("binding.verify", {
            let service = service.clone();
            move |params| {
                require_runtime_write(&params)?;
                let operator = required_str(&params, "operator_user_id")?;
                let platform = optional_str(&params, "platform").unwrap_or_else(|| "web".into());
                let target = parse_target(&params)?;
                let result = service
                    .bind_verify(&operator, &platform, target)
                    .map_err(map_bili_error)?;
                Ok(serde_json::to_value(result).unwrap_or_default())
            }
        });

        ctx.register("binding.unbind", {
            let service = service.clone();
            move |params| {
                require_runtime_write(&params)?;
                let operator = required_str(&params, "operator_user_id")?;
                let removed = service.unbind(&operator).map_err(map_bili_error)?;
                Ok(json!({ "removed": removed }))
            }
        });

        Ok(())
    }

    fn register_events(
        &self,
        _ctx: &mut mutsuki_web_extension::EventRegistry,
    ) -> Result<(), ExtensionError> {
        Ok(())
    }
}

pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    let js = include_str!("../assets/index.js");
    std::fs::write(out_dir.join("index.js"), js)?;
    let assets = vec![AssetEntry {
        path: "index.js".into(),
        content_hash: content_hash(js.as_bytes()),
        bytes: js.len() as u64,
    }];
    std::fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&ExtensionManifest {
            manifest_version: EXTENSION_MANIFEST_VERSION,
            id: PLUGIN_ID.into(),
            version: PLUGIN_VERSION.into(),
            entry: "index.js".into(),
            capabilities: vec![
                CAPABILITY_RUNTIME_READ.into(),
                CAPABILITY_RUNTIME_WRITE.into(),
            ],
            permissions: vec!["pages".into(), "navigation".into()],
            assets,
            protocol_version: WEB_PROTOCOL_VERSION.into(),
        })
        .expect("manifest"),
    )?;
    Ok(out_dir.to_path_buf())
}

fn load_or_synthesize_manifest(root: &Path) -> Result<ExtensionManifest, ExtensionError> {
    let path = root.join("manifest.json");
    if path.exists() {
        let bytes = std::fs::read(&path).map_err(|e| ExtensionError::Manifest(e.to_string()))?;
        return serde_json::from_slice(&bytes).map_err(|e| ExtensionError::Manifest(e.to_string()));
    }
    let bytes = std::fs::read(root.join("index.js"))
        .map_err(|e| ExtensionError::Manifest(e.to_string()))?;
    Ok(ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![
            CAPABILITY_RUNTIME_READ.into(),
            CAPABILITY_RUNTIME_WRITE.into(),
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

fn map_bili_error(error: mutsuki_plugin_bot_bilibili::BilibiliError) -> ExtensionError {
    ExtensionError::Registration(error.to_string())
}

fn require_capability(params: &JsonValue, required: &str) -> Result<(), ExtensionError> {
    let caps = caps_from_params(params);
    if caps.iter().any(|cap| cap == "*" || cap == required) {
        return Ok(());
    }
    Err(ExtensionError::Registration(format!(
        "capability denied: {required}"
    )))
}

fn require_runtime_read(params: &JsonValue) -> Result<(), ExtensionError> {
    require_capability(params, CAPABILITY_RUNTIME_READ)
}

fn require_runtime_write(params: &JsonValue) -> Result<(), ExtensionError> {
    require_capability(params, CAPABILITY_RUNTIME_WRITE)
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
        .unwrap_or_default()
}

fn required_str(params: &JsonValue, key: &str) -> Result<String, ExtensionError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ExtensionError::Registration(format!("missing {key}")))
}

fn optional_str(params: &JsonValue, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn required_u64(params: &JsonValue, key: &str) -> Result<u64, ExtensionError> {
    params
        .get(key)
        .and_then(|v| v.as_u64())
        .filter(|value| *value > 0)
        .ok_or_else(|| ExtensionError::Registration(format!("missing or invalid {key}")))
}

fn parse_notifications_json(params: &JsonValue) -> Result<Vec<BilibiliPollKind>, ExtensionError> {
    let Some(array) = params.get("notifications").and_then(|v| v.as_array()) else {
        return Ok(vec![
            BilibiliPollKind::Live,
            BilibiliPollKind::Dynamic,
            BilibiliPollKind::Video,
        ]);
    };
    let mut out = Vec::new();
    for item in array {
        let kind = match item.as_str().unwrap_or_default() {
            "live" => BilibiliPollKind::Live,
            "dynamic" => BilibiliPollKind::Dynamic,
            "video" => BilibiliPollKind::Video,
            other => {
                return Err(ExtensionError::Registration(format!(
                    "unknown notification type {other}"
                )));
            }
        };
        if !out.contains(&kind) {
            out.push(kind);
        }
    }
    if out.is_empty() {
        return Err(ExtensionError::Registration(
            "notifications must not be empty".into(),
        ));
    }
    Ok(out)
}

fn parse_target(params: &JsonValue) -> Result<BotTarget, ExtensionError> {
    let target = params
        .get("target")
        .cloned()
        .ok_or_else(|| ExtensionError::Registration("missing target".into()))?;
    serde_json::from_value(target)
        .map_err(|error| ExtensionError::Registration(format!("invalid target: {error}")))
}
