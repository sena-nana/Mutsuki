//! Credential Broker: Host secret boundary + login/import/refresh/revoke/status.
//!
//! Model Adapters receive short-lived handles and resolve secret material only through
//! this broker. Plaintext never enters task/event/profile/trace payloads.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mutsuki_agent_contracts::{
    ANTHROPIC_CREDENTIAL_PROVIDER_ID, AgentError, AgentResult, BEDROCK_CREDENTIAL_PROVIDER_ID,
    CREDENTIAL_ACCOUNT_DISABLED, CREDENTIAL_EXPIRED, CREDENTIAL_HANDLE_EXPIRED,
    CREDENTIAL_INSUFFICIENT_SCOPE, CREDENTIAL_INVALID, CREDENTIAL_REVOKED, CREDENTIAL_UNAVAILABLE,
    CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME, CredentialAccessHandle, CredentialCapability,
    CredentialDescriptor, CredentialImportRequest, CredentialImportResult,
    CredentialIssueHandleRequest, CredentialIssueHandleResult, CredentialKind,
    CredentialLifecycleEvent, CredentialLoginRequest, CredentialLoginResult,
    CredentialMaterialOrigin, CredentialProviderDescriptor, CredentialRef, CredentialRefreshPolicy,
    CredentialRefreshRequest, CredentialRefreshResult, CredentialRevocationInfo,
    CredentialRevokeRequest, CredentialRevokeResult, CredentialStatus, CredentialStatusRequest,
    CredentialStatusResult, OPENAI_CREDENTIAL_PROVIDER_ID, VERTEX_CREDENTIAL_PROVIDER_ID,
    official_credential_providers,
};
use serde_json::Value;

/// Host-owned secret persistence boundary. AgentKit never serializes these values.
pub trait SecretStore: Send + Sync {
    fn put(&self, secret_id: &str, material: &str) -> AgentResult<()>;
    fn get(&self, secret_id: &str) -> AgentResult<Option<String>>;
    fn delete(&self, secret_id: &str) -> AgentResult<()>;
}

/// In-memory secret store for tests and Hosts that inject a real keyring behind this trait.
#[derive(Clone, Default)]
pub struct InMemorySecretStore {
    inner: Arc<Mutex<BTreeMap<String, String>>>,
}

impl SecretStore for InMemorySecretStore {
    fn put(&self, secret_id: &str, material: &str) -> AgentResult<()> {
        self.inner
            .lock()
            .expect("secret store mutex")
            .insert(secret_id.to_string(), material.to_string());
        Ok(())
    }

    fn get(&self, secret_id: &str) -> AgentResult<Option<String>> {
        Ok(self
            .inner
            .lock()
            .expect("secret store mutex")
            .get(secret_id)
            .cloned())
    }

    fn delete(&self, secret_id: &str) -> AgentResult<()> {
        self.inner
            .lock()
            .expect("secret store mutex")
            .remove(secret_id);
        Ok(())
    }
}

#[derive(Clone)]
struct StoredCredential {
    descriptor: CredentialDescriptor,
    secret_id: String,
}

#[derive(Clone)]
struct IssuedHandle {
    handle: CredentialAccessHandle,
    secret_material: String,
}

#[derive(Clone)]
pub struct CredentialBrokerService {
    inner: Arc<CredentialBrokerInner>,
}

struct CredentialBrokerInner {
    secrets: Arc<dyn SecretStore>,
    credentials: Mutex<BTreeMap<String, StoredCredential>>,
    handles: Mutex<BTreeMap<String, IssuedHandle>>,
    providers: BTreeMap<String, CredentialProviderDescriptor>,
    next_id: AtomicU64,
    clock_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl Default for CredentialBrokerService {
    fn default() -> Self {
        Self::new(Arc::new(InMemorySecretStore::default()))
    }
}

impl CredentialBrokerService {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self::with_clock(secrets, Arc::new(unix_now_ms))
    }

    pub fn with_clock(
        secrets: Arc<dyn SecretStore>,
        clock_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        let providers = official_credential_providers()
            .into_iter()
            .map(|provider| (provider.provider_id.clone(), provider))
            .collect();
        Self {
            inner: Arc::new(CredentialBrokerInner {
                secrets,
                credentials: Mutex::new(BTreeMap::new()),
                handles: Mutex::new(BTreeMap::new()),
                providers,
                next_id: AtomicU64::new(1),
                clock_ms,
            }),
        }
    }

    pub fn providers(&self) -> Vec<CredentialProviderDescriptor> {
        self.inner.providers.values().cloned().collect()
    }

    pub fn login(&self, request: CredentialLoginRequest) -> AgentResult<CredentialLoginResult> {
        let classified = classify_material(
            &request.provider_id,
            request.kind,
            &request.secret_material,
            request.source.as_deref(),
            CredentialMaterialOrigin::UserProvided,
        )?;
        let descriptor = self.store_credential(
            request.provider_id,
            classified.kind,
            &request.secret_material,
            request.account_label,
            request.source,
            None,
            None,
            classified.capability.or_override(request.capability),
            request.refresh_policy,
            request.expires_at_unix_ms,
            request.metadata,
        )?;
        let event = CredentialLifecycleEvent::Login {
            credential: descriptor.credential.clone(),
            provider_id: descriptor.provider_id.clone(),
            kind: descriptor.kind,
            status: descriptor.status,
        };
        Ok(CredentialLoginResult { descriptor, event })
    }

    pub fn import(&self, request: CredentialImportRequest) -> AgentResult<CredentialImportResult> {
        let classified = classify_material(
            &request.provider_id,
            request.kind,
            &request.secret_material,
            request.source.as_deref(),
            request.origin.clone(),
        )?;
        let descriptor = self.store_credential(
            request.provider_id,
            classified.kind,
            &request.secret_material,
            request.account_label,
            request.source.clone(),
            request.permissions_summary,
            request.independent_revoke_uri,
            classified.capability.or_override(request.capability),
            request.refresh_policy,
            request.expires_at_unix_ms,
            request.metadata,
        )?;
        let event = CredentialLifecycleEvent::Import {
            credential: descriptor.credential.clone(),
            provider_id: descriptor.provider_id.clone(),
            kind: descriptor.kind,
            status: descriptor.status,
            origin: request.origin,
            source: request.source,
        };
        Ok(CredentialImportResult { descriptor, event })
    }

    pub fn refresh(
        &self,
        request: CredentialRefreshRequest,
    ) -> AgentResult<CredentialRefreshResult> {
        let now = (self.inner.clock_ms)();
        let mut credentials = self.inner.credentials.lock().expect("credential mutex");
        let stored = credentials
            .get_mut(&request.credential.credential_id)
            .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
        if stored.descriptor.credential.revision != request.credential.revision
            && !request.force
            && request.credential.revision != 0
        {
            return Err(AgentError::invalid_input(
                "credential revision mismatch; pass force=true to refresh latest",
            ));
        }
        match stored.descriptor.status {
            CredentialStatus::Revoked => {
                return Err(AgentError::new(CREDENTIAL_REVOKED, "credential is revoked"));
            }
            CredentialStatus::UnsupportedForCustomRuntime => {
                return Err(AgentError::new(
                    CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME,
                    "credential cannot be used by AgentKit custom runtime",
                ));
            }
            CredentialStatus::AccountDisabled => {
                return Err(AgentError::new(
                    CREDENTIAL_ACCOUNT_DISABLED,
                    "account is disabled",
                ));
            }
            _ => {}
        }
        if !stored.descriptor.refresh_policy.refreshable {
            // API keys do not rotate in-place; status probe only.
            stored.descriptor.status = evaluate_expiry(stored.descriptor.expires_at_unix_ms, now);
            let event = CredentialLifecycleEvent::Refresh {
                credential: stored.descriptor.credential.clone(),
                status: stored.descriptor.status,
                revision: stored.descriptor.credential.revision,
            };
            return Ok(CredentialRefreshResult {
                descriptor: stored.descriptor.clone(),
                event,
            });
        }
        // Cloud identity / OAuth refresh is Host-assisted: bump revision and mark active.
        stored.descriptor.credential.revision += 1;
        stored.descriptor.status = CredentialStatus::Active;
        if let Some(ttl) = stored
            .descriptor
            .refresh_policy
            .refresh_before_expiry_ms
            .filter(|value| *value > 0)
        {
            stored.descriptor.expires_at_unix_ms = Some(now.saturating_add(ttl.saturating_mul(10)));
        }
        let event = CredentialLifecycleEvent::Refresh {
            credential: stored.descriptor.credential.clone(),
            status: stored.descriptor.status,
            revision: stored.descriptor.credential.revision,
        };
        Ok(CredentialRefreshResult {
            descriptor: stored.descriptor.clone(),
            event,
        })
    }

    pub fn revoke(&self, request: CredentialRevokeRequest) -> AgentResult<CredentialRevokeResult> {
        let now = (self.inner.clock_ms)();
        let mut credentials = self.inner.credentials.lock().expect("credential mutex");
        let stored = credentials
            .get_mut(&request.credential.credential_id)
            .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
        stored.descriptor.status = CredentialStatus::Revoked;
        let independent_revoke_uri = stored.descriptor.independent_revoke_uri.clone();
        stored.descriptor.revocation = Some(CredentialRevocationInfo {
            revoked_at_unix_ms: now,
            reason: request.reason.clone(),
            independent_revoke_uri,
        });
        self.inner.secrets.delete(&stored.secret_id)?;
        let event = CredentialLifecycleEvent::Revoke {
            credential: stored.descriptor.credential.clone(),
            status: CredentialStatus::Revoked,
            reason: request.reason,
        };
        Ok(CredentialRevokeResult {
            descriptor: stored.descriptor.clone(),
            event,
        })
    }

    pub fn status(&self, request: CredentialStatusRequest) -> AgentResult<CredentialStatusResult> {
        let now = (self.inner.clock_ms)();
        let mut credentials = self.inner.credentials.lock().expect("credential mutex");
        let stored = credentials
            .get_mut(&request.credential.credential_id)
            .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
        if stored.descriptor.status == CredentialStatus::Active {
            stored.descriptor.status = evaluate_expiry(stored.descriptor.expires_at_unix_ms, now);
        }
        let event = CredentialLifecycleEvent::Status {
            credential: stored.descriptor.credential.clone(),
            status: stored.descriptor.status,
            revision: stored.descriptor.credential.revision,
        };
        Ok(CredentialStatusResult {
            descriptor: stored.descriptor.clone(),
            event,
        })
    }

    pub fn issue_handle(
        &self,
        request: CredentialIssueHandleRequest,
    ) -> AgentResult<CredentialIssueHandleResult> {
        let now = (self.inner.clock_ms)();
        let credentials = self.inner.credentials.lock().expect("credential mutex");
        let stored = credentials
            .get(&request.credential.credential_id)
            .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
        ensure_usable(&stored.descriptor, now)?;
        let material =
            self.inner.secrets.get(&stored.secret_id)?.ok_or_else(|| {
                AgentError::new(CREDENTIAL_UNAVAILABLE, "secret material missing")
            })?;
        let handle_id = format!(
            "handle-{}",
            self.inner.next_id.fetch_add(1, Ordering::Relaxed)
        );
        let handle = CredentialAccessHandle {
            handle_id: handle_id.clone(),
            credential: stored.descriptor.credential.clone(),
            issued_at_unix_ms: now,
            expires_at_unix_ms: now.saturating_add(request.ttl_ms.max(1)),
        };
        self.inner.handles.lock().expect("handle mutex").insert(
            handle_id,
            IssuedHandle {
                handle: handle.clone(),
                secret_material: material,
            },
        );
        Ok(CredentialIssueHandleResult { handle })
    }

    /// Resolve a short-lived handle into request auth material for Model Adapters.
    pub fn resolve_handle_secret(&self, handle_id: &str) -> AgentResult<String> {
        let now = (self.inner.clock_ms)();
        let mut handles = self.inner.handles.lock().expect("handle mutex");
        let issued = handles
            .get(handle_id)
            .ok_or_else(|| AgentError::new(CREDENTIAL_HANDLE_EXPIRED, "handle not found"))?;
        if issued.handle.expires_at_unix_ms <= now {
            handles.remove(handle_id);
            return Err(AgentError::new(
                CREDENTIAL_HANDLE_EXPIRED,
                "credential access handle expired",
            ));
        }
        // Ensure underlying credential is still usable.
        let credentials = self.inner.credentials.lock().expect("credential mutex");
        let stored = credentials
            .get(&issued.handle.credential.credential_id)
            .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
        ensure_usable(&stored.descriptor, now)?;
        Ok(issued.secret_material.clone())
    }

    /// Resolve by CredentialRef for adapters that still use the legacy broker surface.
    pub fn resolve_secret(&self, credential: &CredentialRef) -> AgentResult<String> {
        let handle = self.issue_handle(CredentialIssueHandleRequest {
            credential: credential.clone(),
            ttl_ms: 60_000,
        })?;
        self.resolve_handle_secret(&handle.handle.handle_id)
    }

    pub fn mark_status(
        &self,
        credential_id: &str,
        status: CredentialStatus,
    ) -> AgentResult<CredentialDescriptor> {
        let mut credentials = self.inner.credentials.lock().expect("credential mutex");
        let stored = credentials
            .get_mut(credential_id)
            .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
        stored.descriptor.status = status;
        Ok(stored.descriptor.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn store_credential(
        &self,
        provider_id: String,
        kind: CredentialKind,
        secret_material: &str,
        account_label: Option<String>,
        source: Option<String>,
        permissions_summary: Option<String>,
        independent_revoke_uri: Option<String>,
        capability: CredentialCapability,
        refresh_policy: CredentialRefreshPolicy,
        expires_at_unix_ms: Option<u64>,
        metadata: Value,
    ) -> AgentResult<CredentialDescriptor> {
        let provider = self.inner.providers.get(&provider_id).ok_or_else(|| {
            AgentError::invalid_input(format!("unknown credential provider `{provider_id}`"))
        })?;
        if !provider.supported_kinds.contains(&kind) {
            return Err(AgentError::invalid_input(format!(
                "provider `{provider_id}` does not support kind `{kind:?}`"
            )));
        }
        if secret_material.trim().is_empty() {
            return Err(AgentError::new(
                CREDENTIAL_INVALID,
                "secret material is empty",
            ));
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let credential_id = format!("cred-{id}");
        let secret_id = format!("secret-{credential_id}");
        self.inner.secrets.put(&secret_id, secret_material)?;
        let descriptor = CredentialDescriptor {
            credential: CredentialRef {
                credential_id: credential_id.clone(),
                revision: 1,
            },
            provider_id,
            kind,
            status: CredentialStatus::Active,
            capability,
            refresh_policy,
            account_label,
            source,
            permissions_summary,
            independent_revoke_uri,
            revocation: None,
            expires_at_unix_ms,
            metadata,
        };
        self.inner
            .credentials
            .lock()
            .expect("credential mutex")
            .insert(
                credential_id,
                StoredCredential {
                    descriptor: descriptor.clone(),
                    secret_id,
                },
            );
        Ok(descriptor)
    }
}

struct ClassifiedMaterial {
    kind: CredentialKind,
    capability: CapabilityOverride,
}

struct CapabilityOverride {
    model_inference: bool,
    media: bool,
    embeddings: bool,
}

impl CapabilityOverride {
    fn or_override(self, mut capability: CredentialCapability) -> CredentialCapability {
        if !capability.model_inference {
            capability.model_inference = self.model_inference;
        }
        if !capability.media {
            capability.media = self.media;
        }
        if !capability.embeddings {
            capability.embeddings = self.embeddings;
        }
        capability
    }
}

fn classify_material(
    provider_id: &str,
    kind: CredentialKind,
    secret_material: &str,
    source: Option<&str>,
    origin: CredentialMaterialOrigin,
) -> AgentResult<ClassifiedMaterial> {
    let material = secret_material.trim();
    let source_lower = source.unwrap_or("").to_ascii_lowercase();

    if is_unsupported_claude_subscription_source(&source_lower)
        || looks_like_claude_code_subscription_token(material)
    {
        return Err(AgentError::new(
            CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME,
            "Claude App/Claude Code subscription credentials are not usable by AgentKit custom runtime",
        ));
    }
    if looks_like_browser_cookie(material) || source_lower.contains("cookie") {
        return Err(AgentError::new(
            CREDENTIAL_INVALID,
            "browser cookies cannot be used as API credentials",
        ));
    }
    if looks_like_codex_private_refresh(material, &source_lower) {
        return Err(AgentError::new(
            CREDENTIAL_INVALID,
            "Codex CLI private refresh tokens are not a supported long-term contract",
        ));
    }
    if kind == CredentialKind::OAuthGrant
        && provider_id == ANTHROPIC_CREDENTIAL_PROVIDER_ID
        && !matches!(origin, CredentialMaterialOrigin::EnterpriseIdentity)
    {
        return Err(AgentError::new(
            CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME,
            "Anthropic OAuth grants are unsupported for custom runtime unless Anthropic publishes third-party use",
        ));
    }

    match provider_id {
        OPENAI_CREDENTIAL_PROVIDER_ID => classify_openai(kind, material, origin),
        ANTHROPIC_CREDENTIAL_PROVIDER_ID => classify_anthropic(kind, material),
        BEDROCK_CREDENTIAL_PROVIDER_ID | VERTEX_CREDENTIAL_PROVIDER_ID => {
            if kind != CredentialKind::CloudIdentity {
                return Err(AgentError::invalid_input(
                    "enterprise providers require cloud_identity credentials",
                ));
            }
            if material.len() < 8 {
                return Err(AgentError::new(
                    CREDENTIAL_INVALID,
                    "cloud identity material is too short",
                ));
            }
            Ok(ClassifiedMaterial {
                kind: CredentialKind::CloudIdentity,
                capability: CapabilityOverride {
                    model_inference: true,
                    media: false,
                    embeddings: true,
                },
            })
        }
        other => Err(AgentError::invalid_input(format!(
            "unsupported credential provider `{other}`"
        ))),
    }
}

fn classify_openai(
    kind: CredentialKind,
    material: &str,
    origin: CredentialMaterialOrigin,
) -> AgentResult<ClassifiedMaterial> {
    if material.starts_with("sk-ant-") {
        return Err(AgentError::new(
            CREDENTIAL_INVALID,
            "Anthropic material cannot be registered under the OpenAI credential provider",
        ));
    }
    if !material.starts_with("sk-") || material.len() < 20 {
        return Err(AgentError::new(
            CREDENTIAL_INVALID,
            "OpenAI API key material is invalid",
        ));
    }
    let resolved_kind = match (kind, origin) {
        (CredentialKind::GeneratedApiKey, _)
        | (_, CredentialMaterialOrigin::OfficialLoginGenerated) => CredentialKind::GeneratedApiKey,
        (CredentialKind::ApiKey, _) => CredentialKind::ApiKey,
        (other, _) => {
            return Err(AgentError::invalid_input(format!(
                "OpenAI provider does not accept kind `{other:?}` for this material"
            )));
        }
    };
    Ok(ClassifiedMaterial {
        kind: resolved_kind,
        capability: CapabilityOverride {
            model_inference: true,
            media: true,
            embeddings: true,
        },
    })
}

fn classify_anthropic(kind: CredentialKind, material: &str) -> AgentResult<ClassifiedMaterial> {
    if kind != CredentialKind::ApiKey {
        return Err(AgentError::invalid_input(
            "Anthropic Console path currently accepts api_key only",
        ));
    }
    if !(material.starts_with("sk-ant-api") || material.starts_with("sk-ant-")) {
        return Err(AgentError::new(
            CREDENTIAL_INVALID,
            "Anthropic Console API key material is invalid",
        ));
    }
    if looks_like_claude_code_subscription_token(material) {
        return Err(AgentError::new(
            CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME,
            "Claude Code subscription token is not a Console API credential",
        ));
    }
    // Reject session/oauth-looking Anthropic tokens that are not Console API keys.
    if material.starts_with("sk-ant-sid-") || material.starts_with("sk-ant-oat-") {
        return Err(AgentError::new(
            CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME,
            "Claude subscription/session tokens are unsupported for custom runtime",
        ));
    }
    if !material.starts_with("sk-ant-api") {
        return Err(AgentError::new(
            CREDENTIAL_INVALID,
            "only Anthropic Console API keys (sk-ant-api…) are accepted",
        ));
    }
    Ok(ClassifiedMaterial {
        kind: CredentialKind::ApiKey,
        capability: CapabilityOverride {
            model_inference: true,
            media: true,
            embeddings: false,
        },
    })
}

fn is_unsupported_claude_subscription_source(source: &str) -> bool {
    matches!(
        source,
        "claude_code"
            | "claude_app"
            | "claude_code_oauth"
            | "claude_pro"
            | "claude_max"
            | "claude_subscription"
            | "claude-code"
            | "claude-app"
    ) || source.contains("claude_code")
        || source.contains("claude-code")
}

fn looks_like_claude_code_subscription_token(material: &str) -> bool {
    let lower = material.to_ascii_lowercase();
    material.starts_with("sk-ant-sid-")
        || material.starts_with("sk-ant-oat-")
        || material.starts_with("eyJ")
        || lower.contains("claude-code")
        || lower.contains("\"https://claude.ai")
}

fn looks_like_browser_cookie(material: &str) -> bool {
    let lower = material.to_ascii_lowercase();
    lower.contains("session-token=")
        || lower.contains("sessiontoken=")
        || lower.contains("__secure-next-auth")
        || (lower.contains("cookie:") && lower.contains("="))
}

fn looks_like_codex_private_refresh(material: &str, source: &str) -> bool {
    source.contains("codex_cli")
        || source.contains("codex-cli")
        || source.contains("auth.json")
        || (material.contains("refresh_token") && material.contains("tokens"))
}

fn evaluate_expiry(expires_at_unix_ms: Option<u64>, now: u64) -> CredentialStatus {
    match expires_at_unix_ms {
        Some(expiry) if expiry <= now => CredentialStatus::Expired,
        _ => CredentialStatus::Active,
    }
}

fn ensure_usable(descriptor: &CredentialDescriptor, now: u64) -> AgentResult<()> {
    match descriptor.status {
        CredentialStatus::Active => {
            if matches!(
                evaluate_expiry(descriptor.expires_at_unix_ms, now),
                CredentialStatus::Expired
            ) {
                return Err(AgentError::new(CREDENTIAL_EXPIRED, "credential expired"));
            }
            Ok(())
        }
        CredentialStatus::Expired => Err(AgentError::new(CREDENTIAL_EXPIRED, "credential expired")),
        CredentialStatus::Revoked => Err(AgentError::new(CREDENTIAL_REVOKED, "credential revoked")),
        CredentialStatus::InsufficientScope => Err(AgentError::new(
            CREDENTIAL_INSUFFICIENT_SCOPE,
            "credential scope is insufficient",
        )),
        CredentialStatus::AccountDisabled => Err(AgentError::new(
            CREDENTIAL_ACCOUNT_DISABLED,
            "account is disabled",
        )),
        CredentialStatus::UnsupportedForCustomRuntime => Err(AgentError::new(
            CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME,
            "credential unsupported for custom runtime",
        )),
        CredentialStatus::PendingRefresh => Err(AgentError::new(
            CREDENTIAL_UNAVAILABLE,
            "credential pending refresh",
        )),
    }
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::CredentialMaterialOrigin;
    use serde_json::json;

    fn openai_key() -> &'static str {
        "sk-test-openai-api-key-0123456789abcdef"
    }

    fn anthropic_key() -> &'static str {
        "sk-ant-api03-console-key-0123456789abcdef"
    }

    #[test]
    fn openai_api_key_login_status_resolve_revoke() {
        let broker = CredentialBrokerService::default();
        let login = broker
            .login(CredentialLoginRequest {
                provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
                kind: CredentialKind::ApiKey,
                secret_material: openai_key().into(),
                account_label: Some("openai-user".into()),
                source: Some("user_api_key".into()),
                capability: CredentialCapability::default(),
                refresh_policy: CredentialRefreshPolicy::default(),
                expires_at_unix_ms: None,
                metadata: json!({}),
            })
            .unwrap();
        assert_eq!(login.descriptor.status, CredentialStatus::Active);
        assert_eq!(login.descriptor.kind, CredentialKind::ApiKey);
        let status = broker
            .status(CredentialStatusRequest {
                credential: login.descriptor.credential.clone(),
            })
            .unwrap();
        assert_eq!(status.descriptor.status, CredentialStatus::Active);
        let secret = broker.resolve_secret(&login.descriptor.credential).unwrap();
        assert_eq!(secret, openai_key());
        let encoded = serde_json::to_string(&status.event).unwrap();
        assert!(!encoded.contains(openai_key()));
        let revoked = broker
            .revoke(CredentialRevokeRequest {
                credential: login.descriptor.credential.clone(),
                reason: Some("rotated".into()),
            })
            .unwrap();
        assert_eq!(revoked.descriptor.status, CredentialStatus::Revoked);
        assert_eq!(
            broker
                .resolve_secret(&login.descriptor.credential)
                .unwrap_err()
                .code,
            CREDENTIAL_REVOKED
        );
    }

    #[test]
    fn openai_generated_api_key_import_is_independent() {
        let broker = CredentialBrokerService::default();
        let imported = broker
            .import(CredentialImportRequest {
                provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
                kind: CredentialKind::GeneratedApiKey,
                secret_material: "sk-proj-generated-from-chatgpt-login-abcdef".into(),
                origin: CredentialMaterialOrigin::OfficialLoginGenerated,
                account_label: Some("chatgpt".into()),
                source: Some("openai_chatgpt_login".into()),
                permissions_summary: Some("API access via generated key".into()),
                independent_revoke_uri: Some("https://platform.openai.com/api-keys".into()),
                capability: CredentialCapability::default(),
                refresh_policy: CredentialRefreshPolicy::default(),
                expires_at_unix_ms: None,
                metadata: json!({}),
            })
            .unwrap();
        assert_eq!(imported.descriptor.kind, CredentialKind::GeneratedApiKey);
        assert!(
            imported
                .descriptor
                .permissions_summary
                .as_deref()
                .is_some_and(|value| value.contains("generated"))
        );
    }

    #[test]
    fn anthropic_console_api_key_round_trip() {
        let broker = CredentialBrokerService::default();
        let login = broker
            .login(CredentialLoginRequest {
                provider_id: ANTHROPIC_CREDENTIAL_PROVIDER_ID.into(),
                kind: CredentialKind::ApiKey,
                secret_material: anthropic_key().into(),
                account_label: None,
                source: Some("anthropic_console".into()),
                capability: CredentialCapability::default(),
                refresh_policy: CredentialRefreshPolicy::default(),
                expires_at_unix_ms: None,
                metadata: Value::Null,
            })
            .unwrap();
        assert_eq!(
            login.descriptor.provider_id,
            ANTHROPIC_CREDENTIAL_PROVIDER_ID
        );
        assert_eq!(
            broker.resolve_secret(&login.descriptor.credential).unwrap(),
            anthropic_key()
        );
    }

    #[test]
    fn claude_code_subscription_token_is_rejected() {
        let broker = CredentialBrokerService::default();
        let error = broker
            .import(CredentialImportRequest {
                provider_id: ANTHROPIC_CREDENTIAL_PROVIDER_ID.into(),
                kind: CredentialKind::ApiKey,
                secret_material: "sk-ant-sid-claude-code-subscription-token".into(),
                origin: CredentialMaterialOrigin::ImportedToolExport,
                account_label: None,
                source: Some("claude_code".into()),
                permissions_summary: None,
                independent_revoke_uri: None,
                capability: CredentialCapability::default(),
                refresh_policy: CredentialRefreshPolicy::default(),
                expires_at_unix_ms: None,
                metadata: Value::Null,
            })
            .unwrap_err();
        assert_eq!(error.code, CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME);
    }

    #[test]
    fn provider_instance_only_stores_credential_ref() {
        let broker = CredentialBrokerService::default();
        let login = broker
            .login(CredentialLoginRequest {
                provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
                kind: CredentialKind::ApiKey,
                secret_material: openai_key().into(),
                account_label: None,
                source: None,
                capability: CredentialCapability::default(),
                refresh_policy: CredentialRefreshPolicy::default(),
                expires_at_unix_ms: None,
                metadata: Value::Null,
            })
            .unwrap();
        let provider = mutsuki_agent_contracts::ProviderInstanceDescriptor {
            provider_id: "openai-prod".into(),
            adapter_id: "openai-compatible".into(),
            endpoint: "https://api.openai.com/v1".into(),
            credential: login.descriptor.credential.clone(),
            models: BTreeMap::new(),
            headers: BTreeMap::new(),
            compatibility: BTreeMap::new(),
            remote_execution_allowed: true,
        };
        let encoded = serde_json::to_string(&provider).unwrap();
        assert!(!encoded.contains(openai_key()));
        assert!(encoded.contains(&login.descriptor.credential.credential_id));
    }

    #[test]
    fn handle_expires_and_login_failure_does_not_break_other_credentials() {
        let clock = Arc::new(AtomicU64::new(1_000));
        let clock_fn = {
            let clock = Arc::clone(&clock);
            Arc::new(move || clock.load(Ordering::Relaxed)) as Arc<dyn Fn() -> u64 + Send + Sync>
        };
        let broker =
            CredentialBrokerService::with_clock(Arc::new(InMemorySecretStore::default()), clock_fn);
        let healthy = broker
            .login(CredentialLoginRequest {
                provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
                kind: CredentialKind::ApiKey,
                secret_material: openai_key().into(),
                account_label: None,
                source: None,
                capability: CredentialCapability::default(),
                refresh_policy: CredentialRefreshPolicy::default(),
                expires_at_unix_ms: None,
                metadata: Value::Null,
            })
            .unwrap();
        assert_eq!(
            broker
                .import(CredentialImportRequest {
                    provider_id: ANTHROPIC_CREDENTIAL_PROVIDER_ID.into(),
                    kind: CredentialKind::ApiKey,
                    secret_material: "sk-ant-sid-bad".into(),
                    origin: CredentialMaterialOrigin::ImportedToolExport,
                    account_label: None,
                    source: Some("claude_code".into()),
                    permissions_summary: None,
                    independent_revoke_uri: None,
                    capability: CredentialCapability::default(),
                    refresh_policy: CredentialRefreshPolicy::default(),
                    expires_at_unix_ms: None,
                    metadata: Value::Null,
                })
                .unwrap_err()
                .code,
            CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME
        );
        let handle = broker
            .issue_handle(CredentialIssueHandleRequest {
                credential: healthy.descriptor.credential.clone(),
                ttl_ms: 10,
            })
            .unwrap();
        clock.store(2_000, Ordering::Relaxed);
        assert_eq!(
            broker
                .resolve_handle_secret(&handle.handle.handle_id)
                .unwrap_err()
                .code,
            CREDENTIAL_HANDLE_EXPIRED
        );
        assert_eq!(
            broker
                .resolve_secret(&healthy.descriptor.credential)
                .unwrap(),
            openai_key()
        );
    }
}
