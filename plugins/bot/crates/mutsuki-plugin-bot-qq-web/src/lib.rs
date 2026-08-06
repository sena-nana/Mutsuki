mod management;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_bot_protocol::{
    AgentSessionBinding, BotCommandDescriptor, BotDeliveryAttempt, BotDeliveryReceipt,
    BotHandlerDescriptor, BotInteractionSession, ConversationPolicy, QqBotCapabilityMatrix,
    QqConversationRef, QqStreamingStrategy,
};

pub use management::{
    LocalQqManagementProvider, QqBotManagementService, QqManagementProvider, account_view_from_config,
    agent_session_view, delivery_view, handler_view,
};
use mutsuki_web_extension::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "qq-bot";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_BOT_READ: &str = "bot.read";
pub const CAPABILITY_BOT_CONFIG_WRITE: &str = "bot.config.write";
pub const CAPABILITY_BOT_DELIVERY_WRITE: &str = "bot.delivery.write";
pub const CAPABILITY_BOT_SESSION_WRITE: &str = "bot.session.write";
pub const CAPABILITY_BOT_SECRET_STATUS: &str = "bot.secret.status";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QqBotManagementSnapshot {
    pub revision: u64,
    pub accounts: Vec<QqAccountView>,
    pub conversations: Vec<QqConversationView>,
    pub handlers: Vec<QqHandlerView>,
    pub commands: Vec<BotCommandDescriptor>,
    pub agent_sessions: Vec<QqAgentSessionView>,
    pub deliveries: Vec<QqDeliveryView>,
    pub interactions: Vec<BotInteractionSession>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqAgentSessionView {
    pub binding: AgentSessionBinding,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn_id: Option<String>,
    pub streaming: QqStreamingStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QqHandlerView {
    pub descriptor: BotHandlerDescriptor,
    pub enabled: bool,
    pub rate_limit_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_invocation_trace_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqAccountView {
    pub account_id: String,
    pub enabled: bool,
    pub health: String,
    pub connection_state: QqGatewayConnectionState,
    pub last_heartbeat_unix_ms: Option<u64>,
    pub intents: u64,
    pub shard: [u64; 2],
    pub credential_reference: String,
    pub credential_status: String,
    pub rate_limit_status: String,
    pub capability: QqBotCapabilityMatrix,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QqGatewayConnectionState {
    Disconnected,
    Connected,
    Identified,
    Resumable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqConversationView {
    pub conversation: QqConversationRef,
    pub policy: ConversationPolicy,
    pub matched_rule_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QqDeliveryView {
    pub receipt: BotDeliveryReceipt,
    pub attempts: Vec<BotDeliveryAttempt>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum QqManagementAction {
    AccountSetEnabled {
        account_id: String,
        enabled: bool,
    },
    AccountHealthCheck {
        account_id: String,
    },
    AccountReconnect {
        account_id: String,
    },
    AccountSendTest {
        account_id: String,
        conversation: QqConversationRef,
        text: String,
    },
    ConversationUpdate {
        conversation: QqConversationRef,
        policy: ConversationPolicy,
    },
    HandlerSetEnabled {
        handler_id: String,
        generation: u64,
        enabled: bool,
    },
    AgentCancel {
        origin_key: String,
        turn_id: String,
    },
    AgentReset {
        origin_key: String,
    },
    AgentFork {
        origin_key: String,
    },
    AgentRegenerate {
        origin_key: String,
    },
    DeliveryRetry {
        delivery_id: String,
    },
    DeliveryCancel {
        delivery_id: String,
    },
    DeliveryPreview {
        delivery_id: String,
    },
    InteractionCancel {
        session_id: String,
    },
}

impl QqManagementAction {
    fn required_capability(&self) -> &'static str {
        match self {
            Self::AccountSetEnabled { .. }
            | Self::AccountHealthCheck { .. }
            | Self::AccountReconnect { .. }
            | Self::AccountSendTest { .. }
            | Self::ConversationUpdate { .. }
            | Self::HandlerSetEnabled { .. } => CAPABILITY_BOT_CONFIG_WRITE,
            Self::AgentCancel { .. }
            | Self::AgentReset { .. }
            | Self::AgentFork { .. }
            | Self::AgentRegenerate { .. }
            | Self::InteractionCancel { .. } => CAPABILITY_BOT_SESSION_WRITE,
            Self::DeliveryRetry { .. }
            | Self::DeliveryCancel { .. }
            | Self::DeliveryPreview { .. } => CAPABILITY_BOT_DELIVERY_WRITE,
        }
    }

    fn requires_confirmation(&self) -> bool {
        !matches!(self, Self::DeliveryPreview { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QqManagementWriteRequest {
    pub actor_id: String,
    pub expected_revision: u64,
    pub action: QqManagementAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QqManagementWriteResult {
    pub revision: u64,
    pub audit_id: String,
    pub result: Value,
}

pub trait QqBotManagementApi: Send + Sync {
    /// Reads the filtered management snapshot, optionally including secret presence metadata.
    ///
    /// # Errors
    ///
    /// Returns a typed management error when the snapshot owner is unavailable or rejects access.
    fn snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<QqBotManagementSnapshot, QqManagementError>;
    /// Applies one revision-fenced management operation.
    ///
    /// # Errors
    ///
    /// Returns a typed management error for stale revisions, policy denial, or owner failure.
    fn write(
        &self,
        request: QqManagementWriteRequest,
    ) -> Result<QqManagementWriteResult, QqManagementError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QqManagementError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for QqManagementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for QqManagementError {}

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
            manifest: load_manifest(root).ok()?,
            root_dir: root.clone(),
        })
    }

    fn register_rpc(&self, registry: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let api = self.api.clone();
        registry.register("snapshot", move |params| {
            require_capability(&params, CAPABILITY_BOT_READ)?;
            let include_secret_status = has_capability(&params, CAPABILITY_BOT_SECRET_STATUS);
            let query = params.get("query").and_then(Value::as_str).unwrap_or("");
            let mut snapshot = api
                .snapshot(query, include_secret_status)
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
        });

        let api = self.api.clone();
        registry.register("write", move |params| {
            let request: QqManagementWriteRequest = serde_json::from_value(
                params
                    .get("request")
                    .cloned()
                    .ok_or_else(|| ExtensionError::Registration("missing request".into()))?,
            )
            .map_err(|error| ExtensionError::Registration(error.to_string()))?;
            if request.actor_id.trim().is_empty() {
                return Err(ExtensionError::Registration("actor_id is required".into()));
            }
            let confirmed = params
                .get("confirmed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if request.action.requires_confirmation() && !confirmed {
                return Err(ExtensionError::Registration(
                    "dangerous action requires confirmation".into(),
                ));
            }
            require_capability(&params, request.action.required_capability())?;
            serde_json::to_value(api.write(request).map_err(domain_error)?).map_err(|error| {
                ExtensionError::Registration(format!("write result encode failed: {error}"))
            })
        });
        Ok(())
    }

    fn register_events(
        &self,
        _registry: &mut mutsuki_web_extension::EventRegistry,
    ) -> Result<(), ExtensionError> {
        Ok(())
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
            CAPABILITY_BOT_SESSION_WRITE.into(),
            CAPABILITY_BOT_SECRET_STATUS.into(),
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

fn has_capability(params: &Value, required: &str) -> bool {
    params
        .get("capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|capability| capability == "*" || capability == required)
}

fn require_capability(params: &Value, required: &str) -> Result<(), ExtensionError> {
    has_capability(params, required)
        .then_some(())
        .ok_or_else(|| ExtensionError::CapabilityDenied(required.into()))
}

fn domain_error(QqManagementError { code, message }: QqManagementError) -> ExtensionError {
    ExtensionError::Registration(json!({"code": code, "message": message}).to_string())
}
