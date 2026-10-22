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
    credential: CredentialRef,
    secret_id: String,
    expires_at_unix_ms: u64,
}

const MAX_ACTIVE_CREDENTIAL_HANDLES: usize = 256;

#[derive(Clone)]
pub struct CredentialBrokerService {
    inner: Arc<CredentialBrokerInner>,
}

struct CredentialBrokerInner {
    secrets: Arc<dyn SecretStore>,
    state: Mutex<CredentialBrokerState>,
    providers: BTreeMap<String, CredentialProviderDescriptor>,
    next_id: AtomicU64,
    clock_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

#[derive(Default)]
struct CredentialBrokerState {
    credentials: BTreeMap<String, StoredCredential>,
    handles: BTreeMap<String, IssuedHandle>,
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
                state: Mutex::new(CredentialBrokerState::default()),
                providers,
                next_id: AtomicU64::new(1),
                clock_ms,
            }),
        }
    }

    pub fn providers(&self) -> Vec<CredentialProviderDescriptor> {
        self.inner.providers.values().cloned().collect()
    }

    /// Restore public credential metadata and its Host-owned secret identifier.
    ///
    /// The descriptor is deliberately secret-free. Active credentials must already
    /// have material in the injected [`SecretStore`]; revoked descriptors may be
    /// restored after their material has been deleted so products retain lifecycle
    /// history without making the credential usable again.
    pub fn restore_descriptor(
        &self,
        descriptor: CredentialDescriptor,
        secret_id: impl Into<String>,
    ) -> AgentResult<()> {
        let secret_id = secret_id.into();
        validate_restored_descriptor(&self.inner.providers, &descriptor, &secret_id)?;
        if descriptor.status != CredentialStatus::Revoked
            && self.inner.secrets.get(&secret_id)?.is_none()
        {
            return Err(AgentError::new(
                CREDENTIAL_UNAVAILABLE,
                "credential secret material is unavailable during restore",
            ));
        }

        let credential_id = descriptor.credential.credential_id.clone();
        let mut state = self.inner.state.lock().expect("credential broker mutex");
        if let Some(existing) = state.credentials.get(&credential_id) {
            if existing.descriptor == descriptor && existing.secret_id == secret_id {
                return Ok(());
            }
            return Err(AgentError::invalid_input(format!(
                "credential `{credential_id}` restore conflicts with existing metadata"
            )));
        }
        if let Some(sequence) = restored_credential_sequence(&credential_id)? {
            self.inner.next_id.fetch_max(sequence, Ordering::Relaxed);
        }
        state.credentials.insert(
            credential_id,
            StoredCredential {
                descriptor,
                secret_id,
            },
        );
        Ok(())
    }

    /// Return the opaque Host secret identifier for persistence metadata.
    /// Secret material never crosses this API.
    pub fn descriptor_secret_id(&self, credential: &CredentialRef) -> AgentResult<String> {
        let state = self.inner.state.lock().expect("credential broker mutex");
        let stored = state
            .credentials
            .get(&credential.credential_id)
            .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
        ensure_credential_revision(
            &stored.descriptor,
            credential,
            "resolving secret identifier",
        )?;
        Ok(stored.secret_id.clone())
    }

    /// Return secret-free current metadata for Host-side persistence recovery.
    pub fn descriptor_by_id(&self, credential_id: &str) -> AgentResult<CredentialDescriptor> {
        self.inner
            .state
            .lock()
            .expect("credential broker mutex")
            .credentials
            .get(credential_id)
            .map(|stored| stored.descriptor.clone())
            .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))
    }

    /// Remove a freshly-created credential when a Host persistence transaction
    /// cannot commit. The secret is deleted before the descriptor is forgotten.
    pub fn discard_for_rollback(&self, credential: &CredentialRef) -> AgentResult<()> {
        let stored = {
            let mut state = self.inner.state.lock().expect("credential broker mutex");
            let stored = state
                .credentials
                .get(&credential.credential_id)
                .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
            ensure_credential_revision(&stored.descriptor, credential, "rollback")?;
            state
                .handles
                .retain(|_, issued| issued.credential.credential_id != credential.credential_id);
            state
                .credentials
                .remove(&credential.credential_id)
                .expect("credential exists after revision validation")
        };
        if let Err(error) = self.inner.secrets.delete(&stored.secret_id) {
            self.inner
                .state
                .lock()
                .expect("credential broker mutex")
                .credentials
                .entry(credential.credential_id.clone())
                .or_insert(stored);
            return Err(error);
        }
        Ok(())
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
        let mut state = self.inner.state.lock().expect("credential broker mutex");
        let stored = state
            .credentials
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
        let (descriptor, secret_id) = {
            let mut state = self.inner.state.lock().expect("credential broker mutex");
            let (descriptor, secret_id) = {
                let stored = state
                    .credentials
                    .get_mut(&request.credential.credential_id)
                    .ok_or_else(|| {
                        AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found")
                    })?;
                ensure_credential_revision(&stored.descriptor, &request.credential, "revoke")?;
                stored.descriptor.credential.revision = stored
                    .descriptor
                    .credential
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| AgentError::invalid_input("credential revision exhausted"))?;
                stored.descriptor.status = CredentialStatus::Revoked;
                let independent_revoke_uri = stored.descriptor.independent_revoke_uri.clone();
                stored.descriptor.revocation = Some(CredentialRevocationInfo {
                    revoked_at_unix_ms: now,
                    reason: request.reason.clone(),
                    independent_revoke_uri,
                });
                (stored.descriptor.clone(), stored.secret_id.clone())
            };
            state.handles.retain(|_, issued| {
                issued.credential.credential_id != request.credential.credential_id
            });
            (descriptor, secret_id)
        };
        self.inner.secrets.delete(&secret_id)?;
        let event = CredentialLifecycleEvent::Revoke {
            credential: descriptor.credential.clone(),
            status: CredentialStatus::Revoked,
            reason: request.reason,
        };
        Ok(CredentialRevokeResult { descriptor, event })
    }

    pub fn status(&self, request: CredentialStatusRequest) -> AgentResult<CredentialStatusResult> {
        let now = (self.inner.clock_ms)();
        let mut state = self.inner.state.lock().expect("credential broker mutex");
        let stored = state
            .credentials
            .get_mut(&request.credential.credential_id)
            .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
        ensure_credential_revision(&stored.descriptor, &request.credential, "status")?;
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
        let checked_at = (self.inner.clock_ms)();
        let secret_id = {
            let state = self.inner.state.lock().expect("credential broker mutex");
            let stored = state
                .credentials
                .get(&request.credential.credential_id)
                .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
            ensure_credential_revision(&stored.descriptor, &request.credential, "issuing handle")?;
            ensure_usable(&stored.descriptor, checked_at)?;
            stored.secret_id.clone()
        };
        if self.inner.secrets.get(&secret_id)?.is_none() {
            return Err(AgentError::new(
                CREDENTIAL_UNAVAILABLE,
                "secret material missing",
            ));
        }
        let issued_at = (self.inner.clock_ms)();
        let handle_id = format!(
            "handle-{}",
            self.inner.next_id.fetch_add(1, Ordering::Relaxed)
        );
        let handle = CredentialAccessHandle {
            handle_id: handle_id.clone(),
            credential: request.credential.clone(),
            issued_at_unix_ms: issued_at,
            expires_at_unix_ms: issued_at.saturating_add(request.ttl_ms.max(1)),
        };
        let mut state = self.inner.state.lock().expect("credential broker mutex");
        let stored = state
            .credentials
            .get(&request.credential.credential_id)
            .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
        ensure_credential_revision(&stored.descriptor, &request.credential, "issuing handle")?;
        ensure_usable(&stored.descriptor, issued_at)?;
        if stored.secret_id != secret_id {
            return Err(AgentError::new(
                CREDENTIAL_UNAVAILABLE,
                "credential secret mapping changed",
            ));
        }
        purge_expired_handles(&mut state.handles, issued_at);
        if state.handles.len() >= MAX_ACTIVE_CREDENTIAL_HANDLES {
            return Err(AgentError::new(
                CREDENTIAL_UNAVAILABLE,
                "credential access handle capacity reached; retry after active handles expire",
            ));
        }
        state.handles.insert(
            handle_id,
            IssuedHandle {
                credential: request.credential,
                secret_id,
                expires_at_unix_ms: handle.expires_at_unix_ms,
            },
        );
        Ok(CredentialIssueHandleResult { handle })
    }

    /// Resolve a short-lived handle into request auth material for Model Adapters.
    pub fn resolve_handle_secret(&self, handle_id: &str) -> AgentResult<String> {
        let now = (self.inner.clock_ms)();
        let (credential, secret_id, expires_at_unix_ms) = {
            let mut state = self.inner.state.lock().expect("credential broker mutex");
            purge_expired_handles(&mut state.handles, now);
            let issued = state
                .handles
                .remove(handle_id)
                .ok_or_else(|| AgentError::new(CREDENTIAL_HANDLE_EXPIRED, "handle not found"))?;
            let stored = state
                .credentials
                .get(&issued.credential.credential_id)
                .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
            ensure_credential_revision(&stored.descriptor, &issued.credential, "resolving handle")?;
            ensure_usable(&stored.descriptor, now)?;
            if stored.secret_id != issued.secret_id {
                return Err(AgentError::new(
                    CREDENTIAL_UNAVAILABLE,
                    "credential secret mapping changed",
                ));
            }
            (
                issued.credential,
                issued.secret_id,
                issued.expires_at_unix_ms,
            )
        };
        let material =
            self.inner.secrets.get(&secret_id)?.ok_or_else(|| {
                AgentError::new(CREDENTIAL_UNAVAILABLE, "secret material missing")
            })?;
        let resolved_at = (self.inner.clock_ms)();
        if expires_at_unix_ms <= resolved_at {
            return Err(AgentError::new(
                CREDENTIAL_HANDLE_EXPIRED,
                "credential access handle expired",
            ));
        }
        let state = self.inner.state.lock().expect("credential broker mutex");
        let stored = state
            .credentials
            .get(&credential.credential_id)
            .ok_or_else(|| AgentError::new(CREDENTIAL_UNAVAILABLE, "credential not found"))?;
        ensure_credential_revision(&stored.descriptor, &credential, "resolving handle")?;
        ensure_usable(&stored.descriptor, resolved_at)?;
        if stored.secret_id != secret_id {
            return Err(AgentError::new(
                CREDENTIAL_UNAVAILABLE,
                "credential secret mapping changed",
            ));
        }
        Ok(material)
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
        let mut state = self.inner.state.lock().expect("credential broker mutex");
        let stored = state
            .credentials
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
            .state
            .lock()
            .expect("credential broker mutex")
            .credentials
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

fn ensure_credential_revision(
    descriptor: &CredentialDescriptor,
    credential: &CredentialRef,
    operation: &str,
) -> AgentResult<()> {
    if descriptor.credential.revision == credential.revision {
        return Ok(());
    }
    Err(AgentError::invalid_input(format!(
        "credential revision mismatch while {operation}: expected {}, received {}",
        descriptor.credential.revision, credential.revision
    )))
}

fn purge_expired_handles(handles: &mut BTreeMap<String, IssuedHandle>, now: u64) {
    handles.retain(|_, issued| issued.expires_at_unix_ms > now);
}

fn validate_restored_descriptor(
    providers: &BTreeMap<String, CredentialProviderDescriptor>,
    descriptor: &CredentialDescriptor,
    secret_id: &str,
) -> AgentResult<()> {
    let credential_id = descriptor.credential.credential_id.trim();
    if credential_id.is_empty() || descriptor.credential.revision == 0 {
        return Err(AgentError::invalid_input(
            "restored credential id and revision must be non-empty",
        ));
    }
    if secret_id.trim().is_empty() {
        return Err(AgentError::invalid_input(
            "restored credential secret identifier must be non-empty",
        ));
    }
    let provider = providers.get(&descriptor.provider_id).ok_or_else(|| {
        AgentError::invalid_input(format!(
            "unknown credential provider `{}` during restore",
            descriptor.provider_id
        ))
    })?;
    if !provider.supported_kinds.contains(&descriptor.kind) {
        return Err(AgentError::invalid_input(format!(
            "provider `{}` does not support restored kind `{:?}`",
            descriptor.provider_id, descriptor.kind
        )));
    }
    Ok(())
}

fn restored_credential_sequence(credential_id: &str) -> AgentResult<Option<u64>> {
    let Some(value) = credential_id.strip_prefix("cred-") else {
        return Ok(None);
    };
    let sequence = value.parse::<u64>().map_err(|_| {
        AgentError::invalid_input(format!(
            "restored credential id `{credential_id}` has an invalid sequence"
        ))
    })?;
    let next = sequence.checked_add(1).ok_or_else(|| {
        AgentError::invalid_input(format!(
            "restored credential id `{credential_id}` exhausts the sequence"
        ))
    })?;
    Ok(Some(next))
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
    use std::sync::atomic::AtomicBool;
    use std::sync::{Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    struct BlockingGetSecretStore {
        inner: InMemorySecretStore,
        block_next_get: AtomicBool,
        get_entered: Mutex<Option<mpsc::Sender<()>>>,
        release_get: Barrier,
    }

    impl BlockingGetSecretStore {
        fn new(get_entered: mpsc::Sender<()>) -> Self {
            Self {
                inner: InMemorySecretStore::default(),
                block_next_get: AtomicBool::new(true),
                get_entered: Mutex::new(Some(get_entered)),
                release_get: Barrier::new(2),
            }
        }
    }

    impl SecretStore for BlockingGetSecretStore {
        fn put(&self, secret_id: &str, material: &str) -> AgentResult<()> {
            self.inner.put(secret_id, material)
        }

        fn get(&self, secret_id: &str) -> AgentResult<Option<String>> {
            if self.block_next_get.swap(false, Ordering::SeqCst) {
                if let Some(sender) = self.get_entered.lock().expect("get-entered mutex").take() {
                    let _ = sender.send(());
                }
                self.release_get.wait();
            }
            self.inner.get(secret_id)
        }

        fn delete(&self, secret_id: &str) -> AgentResult<()> {
            self.inner.delete(secret_id)
        }
    }

    #[derive(Default)]
    struct DeleteFailingSecretStore {
        inner: InMemorySecretStore,
    }

    impl SecretStore for DeleteFailingSecretStore {
        fn put(&self, secret_id: &str, material: &str) -> AgentResult<()> {
            self.inner.put(secret_id, material)
        }

        fn get(&self, secret_id: &str) -> AgentResult<Option<String>> {
            self.inner.get(secret_id)
        }

        fn delete(&self, _secret_id: &str) -> AgentResult<()> {
            Err(AgentError::new(
                CREDENTIAL_UNAVAILABLE,
                "secret store delete failed",
            ))
        }
    }

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
                .resolve_secret(&revoked.descriptor.credential)
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

    #[test]
    fn secret_store_get_does_not_hold_broker_state_lock() {
        let (get_entered_tx, get_entered_rx) = mpsc::channel();
        let secrets = Arc::new(BlockingGetSecretStore::new(get_entered_tx));
        let broker = CredentialBrokerService::new(secrets.clone());
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

        let issue_broker = broker.clone();
        let issue_credential = login.descriptor.credential.clone();
        let issue_thread = thread::spawn(move || {
            issue_broker.issue_handle(CredentialIssueHandleRequest {
                credential: issue_credential,
                ttl_ms: 1_000,
            })
        });
        get_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("issue_handle should reach SecretStore::get");

        let status_broker = broker.clone();
        let status_credential = login.descriptor.credential;
        let (status_tx, status_rx) = mpsc::channel();
        let status_thread = thread::spawn(move || {
            let result = status_broker.status(CredentialStatusRequest {
                credential: status_credential,
            });
            let _ = status_tx.send(result);
        });
        let status_before_secret_release = status_rx.recv_timeout(Duration::from_secs(1));

        secrets.release_get.wait();
        let issue_result = issue_thread.join().expect("issue thread should not panic");
        status_thread
            .join()
            .expect("status thread should not panic");

        assert!(
            status_before_secret_release
                .expect("status must not wait for SecretStore::get")
                .is_ok()
        );
        assert!(issue_result.is_ok());
    }

    #[test]
    fn handles_are_secret_free_one_shot_and_globally_cleaned() {
        let clock = Arc::new(AtomicU64::new(1_000));
        let clock_fn = {
            let clock = Arc::clone(&clock);
            Arc::new(move || clock.load(Ordering::Relaxed)) as Arc<dyn Fn() -> u64 + Send + Sync>
        };
        let broker =
            CredentialBrokerService::with_clock(Arc::new(InMemorySecretStore::default()), clock_fn);
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
        let issue = |ttl_ms| {
            broker
                .issue_handle(CredentialIssueHandleRequest {
                    credential: login.descriptor.credential.clone(),
                    ttl_ms,
                })
                .unwrap()
                .handle
        };

        let one_shot = issue(100);
        {
            let state = broker.inner.state.lock().expect("credential broker mutex");
            let issued = state.handles.get(&one_shot.handle_id).unwrap();
            assert_eq!(issued.credential, login.descriptor.credential);
            assert_ne!(issued.secret_id, openai_key());
            assert!(!issued.secret_id.contains(openai_key()));
        }
        assert_eq!(
            broker.resolve_handle_secret(&one_shot.handle_id).unwrap(),
            openai_key()
        );
        assert_eq!(
            broker
                .resolve_handle_secret(&one_shot.handle_id)
                .unwrap_err()
                .code,
            CREDENTIAL_HANDLE_EXPIRED
        );

        let expired_one = issue(10);
        let _expired_two = issue(10);
        clock.store(2_000, Ordering::Relaxed);
        assert_eq!(
            broker
                .resolve_handle_secret(&expired_one.handle_id)
                .unwrap_err()
                .code,
            CREDENTIAL_HANDLE_EXPIRED
        );
        assert!(
            broker
                .inner
                .state
                .lock()
                .expect("credential broker mutex")
                .handles
                .is_empty()
        );

        clock.store(3_000, Ordering::Relaxed);
        for _ in 0..MAX_ACTIVE_CREDENTIAL_HANDLES {
            let _ = issue(10_000);
        }
        assert_eq!(
            broker
                .issue_handle(CredentialIssueHandleRequest {
                    credential: login.descriptor.credential.clone(),
                    ttl_ms: 10_000,
                })
                .unwrap_err()
                .code,
            CREDENTIAL_UNAVAILABLE
        );
        clock.store(20_000, Ordering::Relaxed);
        assert!(
            broker
                .issue_handle(CredentialIssueHandleRequest {
                    credential: login.descriptor.credential.clone(),
                    ttl_ms: 10_000,
                })
                .is_ok()
        );

        let descriptor = broker
            .descriptor_by_id(&login.descriptor.credential.credential_id)
            .unwrap();
        assert_eq!(descriptor, login.descriptor);
        assert!(
            !serde_json::to_string(&descriptor)
                .unwrap()
                .contains(openai_key())
        );
    }

    #[test]
    fn revoke_rejects_stale_revisions_and_survives_secret_delete_failure() {
        let secrets = Arc::new(DeleteFailingSecretStore::default());
        let broker = CredentialBrokerService::new(secrets.clone());
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
        let handle = broker
            .issue_handle(CredentialIssueHandleRequest {
                credential: login.descriptor.credential.clone(),
                ttl_ms: 1_000,
            })
            .unwrap();

        assert_eq!(
            broker
                .revoke(CredentialRevokeRequest {
                    credential: login.descriptor.credential.clone(),
                    reason: Some("compromised".into()),
                })
                .unwrap_err()
                .code,
            CREDENTIAL_UNAVAILABLE
        );
        let revoked = broker
            .descriptor_by_id(&login.descriptor.credential.credential_id)
            .unwrap();
        assert_eq!(revoked.status, CredentialStatus::Revoked);
        assert_eq!(
            revoked.credential.revision,
            login.descriptor.credential.revision + 1
        );
        assert!(revoked.revocation.is_some());
        assert_eq!(
            broker
                .resolve_handle_secret(&handle.handle.handle_id)
                .unwrap_err()
                .code,
            CREDENTIAL_HANDLE_EXPIRED
        );

        for error in [
            broker
                .status(CredentialStatusRequest {
                    credential: login.descriptor.credential.clone(),
                })
                .unwrap_err(),
            broker
                .issue_handle(CredentialIssueHandleRequest {
                    credential: login.descriptor.credential.clone(),
                    ttl_ms: 1_000,
                })
                .unwrap_err(),
            broker
                .revoke(CredentialRevokeRequest {
                    credential: login.descriptor.credential.clone(),
                    reason: None,
                })
                .unwrap_err(),
        ] {
            assert_eq!(error.code, "agent.invalid_input");
        }

        assert_eq!(
            broker
                .status(CredentialStatusRequest {
                    credential: revoked.credential.clone(),
                })
                .unwrap()
                .descriptor
                .status,
            CredentialStatus::Revoked
        );
        assert_eq!(
            broker
                .issue_handle(CredentialIssueHandleRequest {
                    credential: revoked.credential,
                    ttl_ms: 1_000,
                })
                .unwrap_err()
                .code,
            CREDENTIAL_REVOKED
        );
    }

    #[test]
    fn restored_descriptor_recovers_secret_mapping_and_advances_ids() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let original = CredentialBrokerService::new(secrets.clone());
        let first = original
            .login(CredentialLoginRequest {
                provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
                kind: CredentialKind::ApiKey,
                secret_material: openai_key().into(),
                account_label: Some("restored".into()),
                source: Some("keyring".into()),
                capability: CredentialCapability::default(),
                refresh_policy: CredentialRefreshPolicy::default(),
                expires_at_unix_ms: None,
                metadata: Value::Null,
            })
            .unwrap();
        let secret_id = original
            .descriptor_secret_id(&first.descriptor.credential)
            .unwrap();

        let restored = CredentialBrokerService::new(secrets);
        restored
            .restore_descriptor(first.descriptor.clone(), secret_id)
            .unwrap();
        assert_eq!(
            restored
                .resolve_secret(&first.descriptor.credential)
                .unwrap(),
            openai_key()
        );
        let second = restored
            .login(CredentialLoginRequest {
                provider_id: ANTHROPIC_CREDENTIAL_PROVIDER_ID.into(),
                kind: CredentialKind::ApiKey,
                secret_material: anthropic_key().into(),
                account_label: None,
                source: None,
                capability: CredentialCapability::default(),
                refresh_policy: CredentialRefreshPolicy::default(),
                expires_at_unix_ms: None,
                metadata: Value::Null,
            })
            .unwrap();
        assert_eq!(first.descriptor.credential.credential_id, "cred-1");
        let second_sequence = second
            .descriptor
            .credential
            .credential_id
            .strip_prefix("cred-")
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(second_sequence > 1);
        assert!(
            !serde_json::to_string(&first.descriptor)
                .unwrap()
                .contains(openai_key())
        );
    }

    #[test]
    fn revoked_descriptor_restores_without_deleted_secret() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let original = CredentialBrokerService::new(secrets.clone());
        let login = original
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
        let secret_id = original
            .descriptor_secret_id(&login.descriptor.credential)
            .unwrap();
        let revoked = original
            .revoke(CredentialRevokeRequest {
                credential: login.descriptor.credential,
                reason: Some("removed".into()),
            })
            .unwrap();
        assert!(secrets.get(&secret_id).unwrap().is_none());

        let restored = CredentialBrokerService::new(secrets);
        restored
            .restore_descriptor(revoked.descriptor.clone(), secret_id)
            .unwrap();
        let status = restored
            .status(CredentialStatusRequest {
                credential: revoked.descriptor.credential,
            })
            .unwrap();
        assert_eq!(status.descriptor.status, CredentialStatus::Revoked);
    }

    #[test]
    fn rollback_discards_descriptor_and_secret_together() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let broker = CredentialBrokerService::new(secrets.clone());
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
        let secret_id = broker
            .descriptor_secret_id(&login.descriptor.credential)
            .unwrap();

        broker
            .discard_for_rollback(&login.descriptor.credential)
            .unwrap();
        assert!(secrets.get(&secret_id).unwrap().is_none());
        assert_eq!(
            broker
                .status(CredentialStatusRequest {
                    credential: login.descriptor.credential,
                })
                .unwrap_err()
                .code,
            CREDENTIAL_UNAVAILABLE
        );
    }
}
