use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use mutsuki_bot_console_protocol::platforms::{
    AccountActionParams, AccountActionResult, PLATFORMS_READ_CAPABILITY,
    PLATFORMS_WRITE_CAPABILITY, PlatformAccountAction, PlatformAccountView, PlatformAdapterKind,
    PlatformAdapterView, PlatformHealth,
};
use mutsuki_bot_management::{PlatformConsoleError, PlatformConsoleProvider};
use mutsuki_bot_web_extension_platforms::PlatformsWebExtension;
use mutsuki_web_extension_api::{RpcCallContext, RpcRegistry, WebExtension};
use serde_json::json;

struct FakeProvider {
    adapter_id: &'static str,
    actions: Arc<AtomicUsize>,
}

#[async_trait]
impl PlatformConsoleProvider for FakeProvider {
    fn adapter(&self) -> PlatformAdapterView {
        PlatformAdapterView {
            adapter_id: self.adapter_id.into(),
            platform: "fake".into(),
            label: "Fake".into(),
            kind: PlatformAdapterKind::Messaging,
            config_provider_id: None,
            account_fields: Vec::new(),
            account_actions: vec![PlatformAccountAction {
                id: "reconnect".into(),
                label: "Reconnect".into(),
                danger: true,
                capability: PLATFORMS_WRITE_CAPABILITY.into(),
            }],
            detail_panels: Vec::new(),
        }
    }

    async fn accounts(&self) -> Result<Vec<PlatformAccountView>, PlatformConsoleError> {
        Ok(vec![PlatformAccountView {
            adapter_id: self.adapter_id.into(),
            account_id: format!("{}-main", self.adapter_id),
            display_name: "main".into(),
            avatar_url: None,
            enabled: true,
            health: PlatformHealth::Ok,
            connection_state: "identified".into(),
            fields: BTreeMap::new(),
            last_error: None,
        }])
    }

    async fn action(
        &self,
        _actor_id: &str,
        _request: AccountActionParams,
    ) -> Result<AccountActionResult, PlatformConsoleError> {
        self.actions.fetch_add(1, Ordering::SeqCst);
        Ok(AccountActionResult::default())
    }
}

fn registry(
    live: Arc<std::sync::Mutex<Vec<&'static str>>>,
    actions: Arc<AtomicUsize>,
) -> RpcRegistry {
    let extension = PlatformsWebExtension::new(Arc::new(move || {
        live.lock()
            .unwrap()
            .iter()
            .map(|adapter_id| {
                Arc::new(FakeProvider {
                    adapter_id,
                    actions: actions.clone(),
                }) as Arc<dyn PlatformConsoleProvider>
            })
            .collect()
    }));
    let mut registry = RpcRegistry::new(extension.namespace());
    extension.register_rpc(&mut registry).unwrap();
    registry
}

#[tokio::test]
async fn adapters_are_resolved_live_and_actions_enforce_their_capability() {
    let live = Arc::new(std::sync::Mutex::new(vec!["qq"]));
    let actions = Arc::new(AtomicUsize::new(0));
    let registry = registry(live.clone(), actions.clone());
    let reader = RpcCallContext::new(&[PLATFORMS_READ_CAPABILITY.into()]);

    let adapters = registry
        .call_async_with_context("platforms.adapters.list", json!({}), reader.clone())
        .await
        .unwrap();
    assert_eq!(adapters.as_array().unwrap().len(), 1);

    // An adapter enabled after the console started appears without re-registration.
    live.lock().unwrap().push("bilibili");
    let accounts = registry
        .call_async_with_context(
            "platforms.accounts.list",
            json!({"adapter_id": "bilibili"}),
            reader.clone(),
        )
        .await
        .unwrap();
    assert_eq!(accounts[0]["account_id"], "bilibili-main");

    let action = json!({"adapter_id": "qq", "account_id": "qq-main", "action": "reconnect"});
    let denied = registry
        .call_async_with_context("platforms.accounts.action", action.clone(), reader)
        .await
        .unwrap_err();
    assert_eq!(denied.rpc_code(), "capability_denied");

    let writer = RpcCallContext::new(&[PLATFORMS_WRITE_CAPABILITY.into()]);
    registry
        .call_async_with_context("platforms.accounts.action", action, writer.clone())
        .await
        .unwrap();
    assert_eq!(actions.load(Ordering::SeqCst), 1);

    live.lock().unwrap().clear();
    let gone = registry
        .call_async_with_context(
            "platforms.accounts.action",
            json!({"adapter_id": "qq", "account_id": "qq-main", "action": "reconnect"}),
            writer,
        )
        .await
        .unwrap_err();
    assert_eq!(gone.rpc_code(), "platform.adapter_unavailable");
}
