//! Lifecycle + command schema parity (CLI path uses same ConfigDescriptor).

use std::sync::Arc;

use mutsuki_bot_config::{
    ConfigAction, ConfigApplyRequest, ConfigContext, ConfigError, ConfigLifecycle, ConfigValue,
    RestartPolicy,
};
use mutsuki_bot_web_console::ControlPluginReloadLifecycle;
use mutsuki_bot_web_console::{ProductConfigOptions, product_config_service_with_options};
use mutsuki_plugin_bot_agent::{BOT_AGENT_CONFIG_PROVIDER_ID, BotAgentConfigHandle};
use mutsuki_plugin_bot_control_web::FixtureControlHandler;

struct FailingLifecycle {
    rollbacks: Arc<std::sync::atomic::AtomicUsize>,
}

impl ConfigLifecycle for FailingLifecycle {
    fn execute(
        &self,
        _provider_id: &str,
        _policy: RestartPolicy,
        _pending: &[ConfigAction],
    ) -> Result<Vec<ConfigAction>, ConfigError> {
        Err(ConfigError::ReloadFailed {
            reason: "injected plugin reload failure".into(),
        })
    }

    fn rollback(
        &self,
        _provider_id: &str,
        _policy: RestartPolicy,
        _completed: &[ConfigAction],
    ) -> Result<(), ConfigError> {
        self.rollbacks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn control_lifecycle_invokes_real_plugin_reload() {
    let fixture = Arc::new(FixtureControlHandler::default());
    let lifecycle = ControlPluginReloadLifecycle::new(fixture.clone(), "fixture");
    let completed = lifecycle
        .execute(
            "mutsuki.bot.command",
            RestartPolicy::PluginReload,
            &[ConfigAction::PluginReloaded],
        )
        .unwrap();
    assert_eq!(completed, vec![ConfigAction::PluginReloaded]);
    assert!(
        fixture
            .mutations
            .lock()
            .unwrap()
            .iter()
            .any(|item| item == "plugin_reload")
    );
}

#[tokio::test]
async fn failed_product_reload_restores_file_live_handle_and_provider_revision() {
    let root = tempfile::tempdir().unwrap();
    let product_path = root.path().join("product.toml");
    std::fs::write(
        &product_path,
        r#"
[service]
profile = "bot"
instance_id = "demo"

[[plugins.configured]]
id = "mutsuki.plugin.bot.agent"
config = { enabled = true, connection_id = "primary", default_profile_id = "stable", streaming = "final_only", max_concurrency = 2, timeout_ms = 10000, max_message_bytes = 1200 }
"#,
    )
    .unwrap();
    let handle = BotAgentConfigHandle::default();
    let rollbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let service = product_config_service_with_options(
        &product_path,
        ProductConfigOptions {
            lifecycle: Some(Arc::new(FailingLifecycle {
                rollbacks: rollbacks.clone(),
            })),
            bot_agent_config: Some(handle.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    let context = ConfigContext::plugin_instance("default");
    let caps = vec!["*".into()];
    let before = service
        .read(BOT_AGENT_CONFIG_PROVIDER_ID, context.clone(), &caps)
        .await
        .unwrap();
    let live_before = handle.versioned_snapshot();
    let mut candidate = before.value.clone();
    candidate.as_object_mut().unwrap().insert(
        "streaming".into(),
        ConfigValue::String("segment_messages".into()),
    );
    candidate
        .as_object_mut()
        .unwrap()
        .insert("max_concurrency".into(), ConfigValue::Integer(8));

    assert!(matches!(
        service
            .apply(
                BOT_AGENT_CONFIG_PROVIDER_ID,
                ConfigApplyRequest {
                    candidate,
                    expected_revision: before.revision,
                    dry_run: false,
                },
                context.clone(),
                &caps,
            )
            .await,
        Err(ConfigError::ReloadFailed { .. })
    ));

    let live_after = handle.versioned_snapshot();
    assert_eq!(live_after.config, live_before.config);
    assert!(live_after.generation > live_before.generation);
    let after = service
        .read(BOT_AGENT_CONFIG_PROVIDER_ID, context, &caps)
        .await
        .unwrap();
    assert_eq!(after, before);
    let persisted: toml::Value =
        toml::from_str(&std::fs::read_to_string(&product_path).unwrap()).unwrap();
    assert_eq!(
        persisted["plugins"]["configured"][0]["config"]["streaming"].as_str(),
        Some("final_only")
    );
    assert_eq!(rollbacks.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(!root.path().join(".product.toml.journal").exists());
}
