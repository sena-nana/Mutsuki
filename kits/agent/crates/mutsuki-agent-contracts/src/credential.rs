use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::CredentialRef;

/// Credential material kinds supported by AgentKit Native Coding Agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    ApiKey,
    OAuthGrant,
    GeneratedApiKey,
    CloudIdentity,
}

/// Capabilities a credential can authorize for protocol adapters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialCapability {
    #[serde(default)]
    pub model_inference: bool,
    #[serde(default)]
    pub media: bool,
    #[serde(default)]
    pub embeddings: bool,
    #[serde(default)]
    pub tool_proxy: bool,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Lifecycle status distinct from generic provider unavailability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    Active,
    Expired,
    Revoked,
    InsufficientScope,
    AccountDisabled,
    UnsupportedForCustomRuntime,
    PendingRefresh,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRefreshPolicy {
    #[serde(default)]
    pub refreshable: bool,
    #[serde(default)]
    pub refresh_before_expiry_ms: Option<u64>,
    #[serde(default)]
    pub max_refresh_attempts: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRevocationInfo {
    pub revoked_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub independent_revoke_uri: Option<String>,
}

/// Descriptor for a credential provider implementation (API key, OAuth, cloud identity).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialProviderDescriptor {
    pub provider_id: String,
    pub display_name: String,
    pub protocol_families: Vec<String>,
    pub supported_kinds: Vec<CredentialKind>,
    #[serde(default)]
    pub supports_browser_login: bool,
    #[serde(default)]
    pub enterprise_identity: bool,
}

/// Public credential metadata. Never contains secret material.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CredentialDescriptor {
    pub credential: CredentialRef,
    pub provider_id: String,
    pub kind: CredentialKind,
    pub status: CredentialStatus,
    pub capability: CredentialCapability,
    pub refresh_policy: CredentialRefreshPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub independent_revoke_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation: Option<CredentialRevocationInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub metadata: Value,
}

/// Short-lived handle issued to Model Adapters. Secret material is never embedded here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialAccessHandle {
    pub handle_id: String,
    pub credential: CredentialRef,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMaterialOrigin {
    UserProvided,
    OfficialLoginGenerated,
    EnterpriseIdentity,
    ImportedToolExport,
}

/// Login registers user-provided or browser-completed credential material.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CredentialLoginRequest {
    pub provider_id: String,
    pub kind: CredentialKind,
    /// Secret material. Accepted only on this request boundary; never re-emitted.
    pub secret_material: String,
    #[serde(default)]
    pub account_label: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub capability: CredentialCapability,
    #[serde(default)]
    pub refresh_policy: CredentialRefreshPolicy,
    #[serde(default)]
    pub expires_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CredentialImportRequest {
    pub provider_id: String,
    pub kind: CredentialKind,
    pub secret_material: String,
    pub origin: CredentialMaterialOrigin,
    #[serde(default)]
    pub account_label: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub permissions_summary: Option<String>,
    #[serde(default)]
    pub independent_revoke_uri: Option<String>,
    #[serde(default)]
    pub capability: CredentialCapability,
    #[serde(default)]
    pub refresh_policy: CredentialRefreshPolicy,
    #[serde(default)]
    pub expires_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRefreshRequest {
    pub credential: CredentialRef,
    #[serde(default)]
    pub force: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRevokeRequest {
    pub credential: CredentialRef,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialStatusRequest {
    pub credential: CredentialRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialIssueHandleRequest {
    pub credential: CredentialRef,
    #[serde(default = "default_handle_ttl_ms")]
    pub ttl_ms: u64,
}

fn default_handle_ttl_ms() -> u64 {
    60_000
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CredentialLoginResult {
    pub descriptor: CredentialDescriptor,
    pub event: CredentialLifecycleEvent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CredentialImportResult {
    pub descriptor: CredentialDescriptor,
    pub event: CredentialLifecycleEvent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CredentialRefreshResult {
    pub descriptor: CredentialDescriptor,
    pub event: CredentialLifecycleEvent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CredentialRevokeResult {
    pub descriptor: CredentialDescriptor,
    pub event: CredentialLifecycleEvent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CredentialStatusResult {
    pub descriptor: CredentialDescriptor,
    pub event: CredentialLifecycleEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialIssueHandleResult {
    pub handle: CredentialAccessHandle,
}

/// Domain events for login/import/refresh/revoke/status. Payloads never include secrets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialLifecycleEvent {
    Login {
        credential: CredentialRef,
        provider_id: String,
        kind: CredentialKind,
        status: CredentialStatus,
    },
    Import {
        credential: CredentialRef,
        provider_id: String,
        kind: CredentialKind,
        status: CredentialStatus,
        origin: CredentialMaterialOrigin,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    Refresh {
        credential: CredentialRef,
        status: CredentialStatus,
        revision: u64,
    },
    Revoke {
        credential: CredentialRef,
        status: CredentialStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Status {
        credential: CredentialRef,
        status: CredentialStatus,
        revision: u64,
    },
}

pub const OPENAI_CREDENTIAL_PROVIDER_ID: &str = "mutsuki.credential.openai";
pub const ANTHROPIC_CREDENTIAL_PROVIDER_ID: &str = "mutsuki.credential.anthropic";
pub const BEDROCK_CREDENTIAL_PROVIDER_ID: &str = "mutsuki.credential.amazon-bedrock";
pub const VERTEX_CREDENTIAL_PROVIDER_ID: &str = "mutsuki.credential.google-vertex";

pub const CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME: &str =
    "credential.unsupported_for_custom_runtime";
pub const CREDENTIAL_INVALID: &str = "credential.invalid";
pub const CREDENTIAL_REVOKED: &str = "credential.revoked";
pub const CREDENTIAL_EXPIRED: &str = "credential.expired";
pub const CREDENTIAL_INSUFFICIENT_SCOPE: &str = "credential.insufficient_scope";
pub const CREDENTIAL_ACCOUNT_DISABLED: &str = "credential.account_disabled";
pub const CREDENTIAL_UNAVAILABLE: &str = "credential.unavailable";
pub const CREDENTIAL_HANDLE_EXPIRED: &str = "credential.handle_expired";
pub const CREDENTIAL_SECRET_LEAK_DENIED: &str = "credential.secret_leak_denied";

pub fn official_credential_providers() -> Vec<CredentialProviderDescriptor> {
    vec![
        CredentialProviderDescriptor {
            provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
            display_name: "OpenAI".into(),
            protocol_families: vec!["openai.chat-completions".into(), "openai.responses".into()],
            supported_kinds: vec![CredentialKind::ApiKey, CredentialKind::GeneratedApiKey],
            supports_browser_login: true,
            enterprise_identity: false,
        },
        CredentialProviderDescriptor {
            provider_id: ANTHROPIC_CREDENTIAL_PROVIDER_ID.into(),
            display_name: "Anthropic Console".into(),
            protocol_families: vec!["anthropic.messages".into()],
            supported_kinds: vec![CredentialKind::ApiKey],
            supports_browser_login: false,
            enterprise_identity: false,
        },
        CredentialProviderDescriptor {
            provider_id: BEDROCK_CREDENTIAL_PROVIDER_ID.into(),
            display_name: "Amazon Bedrock".into(),
            protocol_families: vec!["anthropic.messages".into(), "amazon.bedrock".into()],
            supported_kinds: vec![CredentialKind::CloudIdentity],
            supports_browser_login: false,
            enterprise_identity: true,
        },
        CredentialProviderDescriptor {
            provider_id: VERTEX_CREDENTIAL_PROVIDER_ID.into(),
            display_name: "Google Vertex AI".into(),
            protocol_families: vec!["anthropic.messages".into(), "google.vertex".into()],
            supported_kinds: vec![CredentialKind::CloudIdentity],
            supports_browser_login: false,
            enterprise_identity: true,
        },
    ]
}
