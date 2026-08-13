//! Auth / Credential Broker conformance (login path + execution path).

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use mutsuki_agent_adapter_api::CredentialBroker;
use mutsuki_agent_contracts::{
    ANTHROPIC_CREDENTIAL_PROVIDER_ID, CREDENTIAL_REVOKED,
    CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME, CredentialCapability, CredentialImportRequest,
    CredentialKind, CredentialLoginRequest, CredentialMaterialOrigin, CredentialRefreshPolicy,
    CredentialRevokeRequest, CredentialStatus, CredentialStatusRequest, ModelCapability,
    OPENAI_CREDENTIAL_PROVIDER_ID, ProviderInstanceDescriptor, official_credential_providers,
};
use mutsuki_agent_runtime::CredentialBrokerService;
use mutsuki_plugin_agent_credential::AdapterCredentialBroker;
use serde_json::{Value, json};

pub fn run_credential_login_conformance() {
    openai_api_key_login_import_status_revoke();
    openai_generated_key_import_without_app_server();
    anthropic_console_api_key_closed_loop();
    claude_code_subscription_is_rejected();
    secrets_never_enter_events_or_provider_descriptors();
}

pub fn run_credential_execution_conformance() {
    adapter_broker_resolves_via_short_lived_handle();
    revoked_credential_blocks_adapter_resolve();
    official_clients_not_required_for_native_request_path();
    provider_and_profile_store_only_credential_ref();
}

pub fn run_auth_conformance() {
    run_credential_login_conformance();
    run_credential_execution_conformance();
}

fn openai_api_key_login_import_status_revoke() {
    let broker = CredentialBrokerService::default();
    let secret = "sk-test-openai-api-key-conformance-abcdef";
    let login = broker
        .login(CredentialLoginRequest {
            provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
            kind: CredentialKind::ApiKey,
            secret_material: secret.into(),
            account_label: Some("conformance".into()),
            source: Some("user_api_key".into()),
            capability: CredentialCapability {
                model_inference: true,
                ..CredentialCapability::default()
            },
            refresh_policy: CredentialRefreshPolicy::default(),
            expires_at_unix_ms: None,
            metadata: Value::Null,
        })
        .expect("openai login");
    assert_eq!(login.descriptor.status, CredentialStatus::Active);
    let status = broker
        .status(CredentialStatusRequest {
            credential: login.descriptor.credential.clone(),
        })
        .expect("status");
    assert_eq!(status.descriptor.status, CredentialStatus::Active);
    assert_eq!(
        broker.resolve_secret(&login.descriptor.credential).unwrap(),
        secret
    );
    let revoked = broker
        .revoke(CredentialRevokeRequest {
            credential: login.descriptor.credential.clone(),
            reason: Some("invalidated".into()),
        })
        .expect("revoke");
    assert_eq!(
        broker
            .resolve_secret(&revoked.descriptor.credential)
            .unwrap_err()
            .code,
        CREDENTIAL_REVOKED
    );
}

fn openai_generated_key_import_without_app_server() {
    let broker = CredentialBrokerService::default();
    let imported = broker
        .import(CredentialImportRequest {
            provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
            kind: CredentialKind::GeneratedApiKey,
            secret_material: "sk-proj-chatgpt-login-generated-key-abcdef".into(),
            origin: CredentialMaterialOrigin::OfficialLoginGenerated,
            account_label: Some("chatgpt".into()),
            source: Some("openai_chatgpt_login".into()),
            permissions_summary: Some("generated API key".into()),
            independent_revoke_uri: Some("https://platform.openai.com/api-keys".into()),
            capability: CredentialCapability::default(),
            refresh_policy: CredentialRefreshPolicy::default(),
            expires_at_unix_ms: None,
            metadata: json!({"app_server_started": false}),
        })
        .expect("generated key import");
    assert_eq!(imported.descriptor.kind, CredentialKind::GeneratedApiKey);
    assert_eq!(
        imported.descriptor.independent_revoke_uri.as_deref(),
        Some("https://platform.openai.com/api-keys")
    );
    assert_eq!(
        imported.descriptor.metadata["app_server_started"],
        json!(false)
    );
}

fn anthropic_console_api_key_closed_loop() {
    let broker = CredentialBrokerService::default();
    let secret = "sk-ant-api03-console-conformance-abcdef012345";
    let login = broker
        .login(CredentialLoginRequest {
            provider_id: ANTHROPIC_CREDENTIAL_PROVIDER_ID.into(),
            kind: CredentialKind::ApiKey,
            secret_material: secret.into(),
            account_label: None,
            source: Some("anthropic_console".into()),
            capability: CredentialCapability::default(),
            refresh_policy: CredentialRefreshPolicy::default(),
            expires_at_unix_ms: None,
            metadata: Value::Null,
        })
        .expect("anthropic login");
    assert_eq!(
        broker.resolve_secret(&login.descriptor.credential).unwrap(),
        secret
    );
    broker
        .revoke(CredentialRevokeRequest {
            credential: login.descriptor.credential,
            reason: Some("test cleanup".into()),
        })
        .expect("anthropic revoke");
}

fn claude_code_subscription_is_rejected() {
    let broker = CredentialBrokerService::default();
    let error = broker
        .import(CredentialImportRequest {
            provider_id: ANTHROPIC_CREDENTIAL_PROVIDER_ID.into(),
            kind: CredentialKind::OAuthGrant,
            secret_material: "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.claude-code".into(),
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
        .expect_err("claude code must fail loud");
    assert_eq!(error.code, CREDENTIAL_UNSUPPORTED_FOR_CUSTOM_RUNTIME);
}

fn secrets_never_enter_events_or_provider_descriptors() {
    let broker = CredentialBrokerService::default();
    let secret = "sk-secret-must-not-leak-into-events-abcdef";
    let login = broker
        .login(CredentialLoginRequest {
            provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
            kind: CredentialKind::ApiKey,
            secret_material: secret.into(),
            account_label: None,
            source: None,
            capability: CredentialCapability::default(),
            refresh_policy: CredentialRefreshPolicy::default(),
            expires_at_unix_ms: None,
            metadata: Value::Null,
        })
        .unwrap();
    let event = serde_json::to_string(&login.event).unwrap();
    let descriptor = serde_json::to_string(&login.descriptor).unwrap();
    assert!(!event.contains(secret));
    assert!(!descriptor.contains(secret));
}

fn adapter_broker_resolves_via_short_lived_handle() {
    let service = CredentialBrokerService::default();
    let login = service
        .login(CredentialLoginRequest {
            provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
            kind: CredentialKind::ApiKey,
            secret_material: "sk-adapter-resolve-conformance-abcdef".into(),
            account_label: None,
            source: None,
            capability: CredentialCapability::default(),
            refresh_policy: CredentialRefreshPolicy::default(),
            expires_at_unix_ms: None,
            metadata: Value::Null,
        })
        .unwrap();
    let adapter = AdapterCredentialBroker::new(service);
    let value = block_on(adapter.resolve(login.descriptor.credential)).expect("resolve");
    assert_eq!(format!("{value:?}"), "CredentialValue(<redacted>)");
    assert_eq!(
        value.expose_secret(),
        "sk-adapter-resolve-conformance-abcdef"
    );
}

fn revoked_credential_blocks_adapter_resolve() {
    let service = CredentialBrokerService::default();
    let login = service
        .login(CredentialLoginRequest {
            provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
            kind: CredentialKind::ApiKey,
            secret_material: "sk-revoked-adapter-conformance-abcdef".into(),
            account_label: None,
            source: None,
            capability: CredentialCapability::default(),
            refresh_policy: CredentialRefreshPolicy::default(),
            expires_at_unix_ms: None,
            metadata: Value::Null,
        })
        .unwrap();
    let revoked = service
        .revoke(CredentialRevokeRequest {
            credential: login.descriptor.credential.clone(),
            reason: Some("invalid".into()),
        })
        .unwrap();
    let adapter = AdapterCredentialBroker::new(service);
    let error = block_on(adapter.resolve(revoked.descriptor.credential)).unwrap_err();
    assert_eq!(error.code, CREDENTIAL_REVOKED);
}

fn official_clients_not_required_for_native_request_path() {
    assert!(
        official_credential_providers()
            .iter()
            .any(|provider| provider.provider_id == OPENAI_CREDENTIAL_PROVIDER_ID)
    );
    assert!(
        official_credential_providers()
            .iter()
            .any(|provider| provider.provider_id == ANTHROPIC_CREDENTIAL_PROVIDER_ID)
    );
    let service = CredentialBrokerService::default();
    let login = service
        .login(CredentialLoginRequest {
            provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
            kind: CredentialKind::ApiKey,
            secret_material: "sk-native-no-official-process-abcdef".into(),
            account_label: None,
            source: None,
            capability: CredentialCapability::default(),
            refresh_policy: CredentialRefreshPolicy::default(),
            expires_at_unix_ms: None,
            metadata: Value::Null,
        })
        .unwrap();
    let provider = ProviderInstanceDescriptor {
        provider_id: "native-openai".into(),
        adapter_id: "openai-compatible".into(),
        endpoint: "https://api.openai.com/v1".into(),
        credential: login.descriptor.credential,
        models: BTreeMap::from([("gpt-test".into(), ModelCapability::default())]),
        headers: BTreeMap::new(),
        compatibility: BTreeMap::new(),
        remote_execution_allowed: true,
    };
    assert!(
        !serde_json::to_string(&provider)
            .unwrap()
            .contains("sk-native")
    );
}

fn provider_and_profile_store_only_credential_ref() {
    let service = CredentialBrokerService::default();
    let login = service
        .login(CredentialLoginRequest {
            provider_id: OPENAI_CREDENTIAL_PROVIDER_ID.into(),
            kind: CredentialKind::ApiKey,
            secret_material: "sk-profile-ref-only-conformance-abcdef".into(),
            account_label: None,
            source: None,
            capability: CredentialCapability::default(),
            refresh_policy: CredentialRefreshPolicy::default(),
            expires_at_unix_ms: None,
            metadata: Value::Null,
        })
        .unwrap();
    let profile_provider = mutsuki_agent_contracts::AgentProviderInstance {
        instance_id: "primary".into(),
        adapter_id: "openai-compatible".into(),
        credential_ref: Some(login.descriptor.credential.clone()),
        capability_tags: vec!["tools".into()],
        endpoint_profile: None,
        test_only: true,
    };
    let encoded = serde_json::to_string(&profile_provider).unwrap();
    assert!(encoded.contains("credential_id"));
    assert!(!encoded.contains("sk-profile-ref-only"));
    let _keep: Arc<AdapterCredentialBroker> = Arc::new(AdapterCredentialBroker::new(service));
}

fn block_on<T>(future: Pin<Box<dyn Future<Output = T> + Send>>) -> T {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = future;
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn auth_conformance_passes() {
        super::run_auth_conformance();
    }
}
