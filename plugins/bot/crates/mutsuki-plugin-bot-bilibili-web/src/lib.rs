//! Bilibili console WebExtension: login state + subscription management RPC.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::redundant_closure_for_method_calls,
    clippy::return_self_not_must_use,
    clippy::too_many_lines,
    clippy::uninlined_format_args
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_bot_management::{
    BilibiliManagementApi, BilibiliManagementError, BilibiliNotificationKind,
};
use mutsuki_bot_protocol::BotTarget;
use mutsuki_web_extension_api::{
    BUNDLED_ENTRY_ASSET, ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor,
    content_hash, load_bundled_manifest,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde_json::{Value as JsonValue, json};

pub const PLUGIN_ID: &str = "bilibili";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_RUNTIME_READ: &str = "runtime.read";
pub const CAPABILITY_RUNTIME_WRITE: &str = "runtime.write";

/// Fixed actor id used for console-initiated QR sessions.
pub const CONSOLE_LOGIN_ACTOR: &str = "web-console";

pub struct BilibiliWebExtension {
    service: Arc<dyn BilibiliManagementApi>,
    assets_root: Option<PathBuf>,
}

impl BilibiliWebExtension {
    pub fn new(service: Arc<dyn BilibiliManagementApi>) -> Self {
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

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let service = self.service.clone();

        ctx.register_contextual("status", {
            let service = service.clone();
            move |context, _params| {
                context.require(CAPABILITY_RUNTIME_READ)?;
                encode_json(service.status())
            }
        });

        ctx.register_async_contextual("login.start", {
            let service = service.clone();
            move |context, params| {
                let service = service.clone();
                async move {
                    context.require(CAPABILITY_RUNTIME_WRITE)?;
                    let actor =
                        optional_str(&params, "actor_id").unwrap_or(CONSOLE_LOGIN_ACTOR.into());
                    let result = service.login_start(&actor).await.map_err(map_bili_error)?;
                    Ok(json!({ "qr_png_base64": result.qr_png_base64 }))
                }
            }
        });

        ctx.register_contextual("login.poll", {
            let service = service.clone();
            move |context, params| {
                context.require(CAPABILITY_RUNTIME_READ)?;
                let actor = optional_str(&params, "actor_id").unwrap_or(CONSOLE_LOGIN_ACTOR.into());
                let result = service.login_poll(&actor).map_err(map_bili_error)?;
                encode_json(result)
            }
        });

        ctx.register_contextual("credential.clear", {
            let service = service.clone();
            move |context, params| {
                context.require(CAPABILITY_RUNTIME_WRITE)?;
                require_confirmed(&params)?;
                service.credential_clear().map_err(map_bili_error)?;
                Ok(json!({ "ok": true }))
            }
        });

        ctx.register_contextual("subscriptions.list", {
            let service = service.clone();
            move |context, params| {
                context.require(CAPABILITY_RUNTIME_READ)?;
                let actor = optional_str(&params, "operator_user_id").unwrap_or_default();
                let is_admin = params
                    .get("is_admin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let list = service.list(&actor, is_admin).map_err(map_bili_error)?;
                Ok(json!({ "subscriptions": list }))
            }
        });

        ctx.register_contextual("subscriptions.subscribe", {
            let service = service.clone();
            move |context, params| {
                context.require(CAPABILITY_RUNTIME_WRITE)?;
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
                encode_json(view)
            }
        });

        ctx.register_contextual("subscriptions.unsubscribe", {
            let service = service.clone();
            move |context, params| {
                context.require(CAPABILITY_RUNTIME_WRITE)?;
                let subscription_id = required_str(&params, "subscription_id")?;
                require_confirmed(&params)?;
                service
                    .unsubscribe(&subscription_id)
                    .map_err(map_bili_error)?;
                Ok(json!({ "ok": true }))
            }
        });

        ctx.register_contextual("subscriptions.set_paused", {
            let service = service.clone();
            move |context, params| {
                context.require(CAPABILITY_RUNTIME_WRITE)?;
                let actor = optional_str(&params, "operator_user_id").unwrap_or_default();
                let is_admin = params
                    .get("is_admin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let selector = optional_str(&params, "selector");
                let paused = params
                    .get("paused")
                    .and_then(|v| v.as_bool())
                    .ok_or_else(|| invalid_argument("missing paused"))?;
                let view = service
                    .set_paused(&actor, is_admin, selector.as_deref(), paused)
                    .map_err(map_bili_error)?;
                encode_json(view)
            }
        });

        ctx.register_contextual("subscriptions.preview", {
            let service = service.clone();
            move |context, params| {
                context.require(CAPABILITY_RUNTIME_READ)?;
                let actor = optional_str(&params, "operator_user_id").unwrap_or_default();
                let is_admin = params
                    .get("is_admin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let selector = optional_str(&params, "selector");
                let card = service
                    .preview(&actor, is_admin, selector.as_deref())
                    .map_err(map_bili_error)?;
                encode_json(card)
            }
        });

        ctx.register_contextual("binding.start", {
            let service = service.clone();
            move |context, params| {
                context.require(CAPABILITY_RUNTIME_WRITE)?;
                let operator = required_str(&params, "operator_user_id")?;
                let uid = required_u64(&params, "uid")?;
                let seed = optional_str(&params, "challenge_seed")
                    .unwrap_or_else(|| format!("web-{}", operator));
                let result = service
                    .bind_start(&operator, uid, &seed)
                    .map_err(map_bili_error)?;
                encode_json(result)
            }
        });

        ctx.register_contextual("binding.verify", {
            let service = service.clone();
            move |context, params| {
                context.require(CAPABILITY_RUNTIME_WRITE)?;
                let operator = required_str(&params, "operator_user_id")?;
                let platform = optional_str(&params, "platform").unwrap_or_else(|| "web".into());
                let target = parse_target(&params)?;
                let result = service
                    .bind_verify(&operator, &platform, target)
                    .map_err(map_bili_error)?;
                encode_json(result)
            }
        });

        ctx.register_contextual("binding.unbind", {
            let service = service.clone();
            move |context, params| {
                context.require(CAPABILITY_RUNTIME_WRITE)?;
                let operator = required_str(&params, "operator_user_id")?;
                let removed = service.unbind(&operator).map_err(map_bili_error)?;
                Ok(json!({ "removed": removed }))
            }
        });

        Ok(())
    }

    fn register_events(
        &self,
        ctx: &mut mutsuki_web_extension_api::EventRegistry,
    ) -> Result<(), ExtensionError> {
        ctx.register_topic("changed");
        Ok(())
    }
}

fn manifest(assets: Vec<AssetEntry>) -> ExtensionManifest {
    ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: BUNDLED_ENTRY_ASSET.into(),
        capabilities: vec![
            CAPABILITY_RUNTIME_READ.into(),
            CAPABILITY_RUNTIME_WRITE.into(),
        ],
        permissions: vec!["pages".into(), "navigation".into()],
        assets,
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    }
}

pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    let js = include_str!("../assets/index.js");
    std::fs::write(out_dir.join(BUNDLED_ENTRY_ASSET), js)?;
    let assets = vec![AssetEntry {
        path: BUNDLED_ENTRY_ASSET.into(),
        content_hash: content_hash(js.as_bytes()),
        bytes: js.len() as u64,
    }];
    std::fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest(assets)).expect("manifest"),
    )?;
    Ok(out_dir.to_path_buf())
}

fn map_bili_error(error: BilibiliManagementError) -> ExtensionError {
    ExtensionError::Rpc {
        code: error.code,
        message: error.message,
    }
}

fn encode_json(value: impl serde::Serialize) -> Result<JsonValue, ExtensionError> {
    serde_json::to_value(value).map_err(|error| ExtensionError::Rpc {
        code: "bilibili.encode_failed".into(),
        message: error.to_string(),
    })
}

fn invalid_argument(message: impl Into<String>) -> ExtensionError {
    ExtensionError::Rpc {
        code: "bilibili.invalid_argument".into(),
        message: message.into(),
    }
}

fn require_confirmed(params: &JsonValue) -> Result<(), ExtensionError> {
    if params.get("confirmed").and_then(JsonValue::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(ExtensionError::Rpc {
            code: "bilibili.confirmation_required".into(),
            message: "destructive action requires confirmation".into(),
        })
    }
}

fn required_str(params: &JsonValue, key: &str) -> Result<String, ExtensionError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_argument(format!("missing {key}")))
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
        .ok_or_else(|| invalid_argument(format!("missing or invalid {key}")))
}

fn parse_notifications_json(
    params: &JsonValue,
) -> Result<Vec<BilibiliNotificationKind>, ExtensionError> {
    let Some(array) = params.get("notifications").and_then(|v| v.as_array()) else {
        return Ok(vec![
            BilibiliNotificationKind::Live,
            BilibiliNotificationKind::Dynamic,
            BilibiliNotificationKind::Video,
        ]);
    };
    let mut out = Vec::new();
    for item in array {
        let kind = match item.as_str().unwrap_or_default() {
            "live" => BilibiliNotificationKind::Live,
            "dynamic" => BilibiliNotificationKind::Dynamic,
            "video" => BilibiliNotificationKind::Video,
            other => {
                return Err(invalid_argument(format!(
                    "unknown notification type {other}"
                )));
            }
        };
        if !out.contains(&kind) {
            out.push(kind);
        }
    }
    if out.is_empty() {
        return Err(invalid_argument("notifications must not be empty"));
    }
    Ok(out)
}

fn parse_target(params: &JsonValue) -> Result<BotTarget, ExtensionError> {
    let target = params
        .get("target")
        .cloned()
        .ok_or_else(|| invalid_argument("missing target"))?;
    serde_json::from_value(target)
        .map_err(|error| invalid_argument(format!("invalid target: {error}")))
}
