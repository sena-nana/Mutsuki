use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use mutsuki_bot_sandbox::*;
use mutsuki_web_extension::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "sandbox";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_BOT_READ: &str = "bot.read";
pub const CAPABILITY_SANDBOX_WRITE: &str = "bot.sandbox.write";
pub const CAPABILITY_BOT_DELIVERY_WRITE: &str = "bot.delivery.write";

const EXTRA_IMG_SRC: &[&str] = &[
    "https://*.qlogo.cn",
    "https://*.qpic.cn",
    "https://*.gtimg.cn",
    "https://*.qq.com.cn",
    "https://*.nt.qq.com.cn",
];

pub struct SandboxWebExtension {
    api: Arc<dyn SandboxApi>,
    assets_root: Option<PathBuf>,
}

impl SandboxWebExtension {
    #[must_use]
    pub fn new(api: Arc<dyn SandboxApi>) -> Self {
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

impl WebExtension for SandboxWebExtension {
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
            manifest: load_manifest(root).ok()?,
            root_dir: root.clone(),
        })
    }

    fn register_rpc(&self, registry: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let api = self.api.clone();
        registry.register_async_contextual("snapshot", move |context, params| {
            let api = api.clone();
            async move {
                context.require(CAPABILITY_BOT_READ)?;
                let query = params.get("query").and_then(Value::as_str).unwrap_or("");
                serde_json::to_value(api.snapshot(query).await.map_err(domain_error)?)
                    .map_err(|error| encode_error(&error))
            }
        });

        let api = self.api.clone();
        registry.register_async_contextual("messages", move |context, params| {
            let api = api.clone();
            async move {
                context.require(CAPABILITY_BOT_READ)?;
                let conversation_id = params
                    .get("conversation_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ExtensionError::Registration("missing conversation_id".into())
                    })?;
                serde_json::to_value(api.messages(conversation_id).await.map_err(domain_error)?)
                    .map_err(|error| encode_error(&error))
            }
        });

        let api = self.api.clone();
        registry.register_async_contextual("write", move |context, params| {
            let api = api.clone();
            async move {
                let mut request: SandboxWriteRequest = serde_json::from_value(
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
                let snapshot = api.snapshot("").await.map_err(domain_error)?;
                if requires_confirmation(snapshot.mode, &request.action) && !confirmed {
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
                .map_err(|error| encode_error(&error))
            }
        });

        let api = self.api.clone();
        registry.register_async_contextual("media.upload", move |context, params| {
            let api = api.clone();
            async move {
                context.require(CAPABILITY_SANDBOX_WRITE)?;
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let mime = params.get("mime").and_then(Value::as_str).unwrap_or("");
                let raw = params
                    .get("bytes")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ExtensionError::Registration("missing bytes".into()))?;
                let bytes = base64_decode(raw)
                    .map_err(|error| ExtensionError::Registration(error.to_owned()))?;
                serde_json::to_value(
                    api.upload_media(name, mime, bytes)
                        .await
                        .map_err(domain_error)?,
                )
                .map_err(|error| encode_error(&error))
            }
        });

        let api = self.api.clone();
        registry.register_async_contextual("media.get", move |context, params| {
            let api = api.clone();
            async move {
                context.require(CAPABILITY_BOT_READ)?;
                let media_id = params
                    .get("media_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ExtensionError::Registration("missing media_id".into()))?;
                let blob = api.media_blob(media_id).await.map_err(domain_error)?;
                Ok(json!({
                    "media_id": blob.media_id,
                    "mime": blob.mime,
                    "name": blob.name,
                    "bytes": base64_encode(&blob.bytes),
                }))
            }
        });

        let api = self.api.clone();
        registry.register_async_contextual("sticker.upload", move |context, params| {
            let api = api.clone();
            async move {
                context.require(CAPABILITY_SANDBOX_WRITE)?;
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let mime = params.get("mime").and_then(Value::as_str).unwrap_or("");
                let raw = params
                    .get("bytes")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ExtensionError::Registration("missing bytes".into()))?;
                let bytes = base64_decode(raw)
                    .map_err(|error| ExtensionError::Registration(error.to_owned()))?;
                let uploaded = api
                    .upload_sticker(name, mime, bytes)
                    .await
                    .map_err(domain_error)?;
                Ok(json!({
                    "sticker_id": uploaded.media_id,
                    "mime": uploaded.mime,
                    "name": uploaded.name,
                }))
            }
        });

        let api = self.api.clone();
        registry.register_async_contextual("sticker.list", move |context, _params| {
            let api = api.clone();
            async move {
                context.require(CAPABILITY_BOT_READ)?;
                serde_json::to_value(api.list_stickers().await.map_err(domain_error)?)
                    .map_err(|error| encode_error(&error))
            }
        });

        let api = self.api.clone();
        registry.register_async_contextual("sticker.get", move |context, params| {
            let api = api.clone();
            async move {
                context.require(CAPABILITY_BOT_READ)?;
                let sticker_id = params
                    .get("sticker_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ExtensionError::Registration("missing sticker_id".into()))?;
                let blob = api.sticker_blob(sticker_id).await.map_err(domain_error)?;
                Ok(json!({
                    "sticker_id": blob.media_id,
                    "mime": blob.mime,
                    "name": blob.name,
                    "bytes": base64_encode(&blob.bytes),
                }))
            }
        });
        Ok(())
    }

    fn register_events(
        &self,
        registry: &mut mutsuki_web_extension::EventRegistry,
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

/// Writes the embedded sandbox frontend and its content-addressed manifest.
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
            CAPABILITY_SANDBOX_WRITE.into(),
            CAPABILITY_BOT_DELIVERY_WRITE.into(),
        ],
        permissions: vec!["pages".into(), "navigation".into()],
        assets,
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    }
}

fn load_manifest(root: &Path) -> Result<ExtensionManifest, ExtensionError> {
    let path = root.join("manifest.json");
    if path.exists() {
        return serde_json::from_slice(
            &std::fs::read(path).map_err(|error| ExtensionError::Manifest(error.to_string()))?,
        )
        .map_err(|error| ExtensionError::Manifest(error.to_string()));
    }
    let bytes = std::fs::read(root.join("index.js"))
        .map_err(|error| ExtensionError::Manifest(error.to_string()))?;
    Ok(manifest(vec![AssetEntry {
        path: "index.js".into(),
        content_hash: content_hash(&bytes),
        bytes: bytes.len() as u64,
    }]))
}

fn domain_error(SandboxError { code, message }: SandboxError) -> ExtensionError {
    ExtensionError::Registration(json!({"code": code, "message": message}).to_string())
}

fn encode_error(error: &serde_json::Error) -> ExtensionError {
    ExtensionError::Registration(format!("encode failed: {error}"))
}

fn required_capability(action: &SandboxAction) -> &'static str {
    match action {
        SandboxAction::SendAsBot { .. } => CAPABILITY_BOT_DELIVERY_WRITE,
        _ => CAPABILITY_SANDBOX_WRITE,
    }
}

fn requires_confirmation(mode: SandboxMode, action: &SandboxAction) -> bool {
    mode == SandboxMode::Live && matches!(action, SandboxAction::SendAsBot { .. })
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(TABLE[((triple >> 18) & 0x3f) as usize]));
        out.push(char::from(TABLE[((triple >> 12) & 0x3f) as usize]));
        if chunk.len() > 1 {
            out.push(char::from(TABLE[((triple >> 6) & 0x3f) as usize]));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(TABLE[(triple & 0x3f) as usize]));
        } else {
            out.push('=');
        }
    }
    out
}

fn base64_decode(input: &str) -> Result<Vec<u8>, &'static str> {
    let input = input.trim();
    if input.len() % 4 != 0 {
        return Err("invalid base64");
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let decode = |ch: u8| -> Result<u8, &'static str> {
        match ch {
            b'A'..=b'Z' => Ok(ch - b'A'),
            b'a'..=b'z' => Ok(ch - b'a' + 26),
            b'0'..=b'9' => Ok(ch - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            b'=' => Ok(0),
            _ => Err("invalid base64"),
        }
    };
    for chunk in input.as_bytes().chunks(4) {
        if chunk.len() != 4 {
            return Err("invalid base64");
        }
        let n0 = u32::from(decode(chunk[0])?);
        let n1 = u32::from(decode(chunk[1])?);
        let n2 = u32::from(decode(chunk[2])?);
        let n3 = u32::from(decode(chunk[3])?);
        let triple = (n0 << 18) | (n1 << 12) | (n2 << 6) | n3;
        out.push((triple >> 16) as u8);
        if chunk[2] != b'=' {
            out.push((triple >> 8) as u8);
        }
        if chunk[3] != b'=' {
            out.push(triple as u8);
        }
    }
    Ok(out)
}
