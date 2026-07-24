//! Lifecycle + command schema parity (CLI path uses same ConfigDescriptor).

use std::sync::{Arc, Mutex};

use mutsuki_bot_config::{
    ConfigAction, ConfigApplyMode, ConfigApplyRequest, ConfigContext, ConfigError, ConfigLifecycle,
    ConfigProviderRegistry, ConfigRevision, ConfigService, ConfigValue, MemoryConfigProvider,
    MutsukiConfigSchema, RestartPolicy,
};
use mutsuki_bot_web_console::ControlPluginReloadLifecycle;
use mutsuki_plugin_bot_command::BotCommandConfig;
use mutsuki_plugin_bot_control_web::FixtureControlHandler;

struct RecordingLifecycle {
    calls: Arc<Mutex<Vec<String>>>,
}

impl ConfigLifecycle for RecordingLifecycle {
    fn execute(
        &self,
        provider_id: &str,
        policy: RestartPolicy,
        pending: &[ConfigAction],
    ) -> Result<Vec<ConfigAction>, ConfigError> {
        self.calls.lock().unwrap().push(format!(
            "{provider_id}:{policy:?}:{}",
            pending
                .iter()
                .map(|action| format!("{action:?}"))
                .collect::<Vec<_>>()
                .join(",")
        ));
        if pending
            .iter()
            .any(|action| matches!(action, ConfigAction::PluginReloaded))
            || matches!(policy, RestartPolicy::PluginReload)
        {
            Ok(vec![ConfigAction::PluginReloaded])
        } else {
            Ok(Vec::new())
        }
    }
}

#[tokio::test]
async fn command_schema_cli_fixture_matches_derive_and_lifecycle_reloads() {
    let schema = BotCommandConfig::schema();
    assert_eq!(schema.provider_id.as_str(), "mutsuki.bot.command");
    assert!(
        schema
            .root
            .children
            .iter()
            .any(|node| node.key.as_str() == "prefixes"
                && matches!(
                    node.value_type,
                    mutsuki_bot_config::ConfigValueType::Array { .. }
                ))
    );

    let calls = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(ConfigProviderRegistry::default());
    registry
        .register(Arc::new(MemoryConfigProvider::new(
            schema,
            ConfigValue::Object(
                [(
                    "prefixes".into(),
                    ConfigValue::Array(vec![ConfigValue::String("/".into())]),
                )]
                .into_iter()
                .collect(),
            ),
            ConfigApplyMode::HotReload,
        )))
        .unwrap();
    let service = ConfigService::new(registry).with_lifecycle(Arc::new(RecordingLifecycle {
        calls: calls.clone(),
    }));
    let caps = vec!["*".into()];
    let ctx = ConfigContext::plugin_instance("default");
    let snap = service
        .read("mutsuki.bot.command", ctx.clone(), &caps)
        .await
        .unwrap();
    let result = service
        .apply(
            "mutsuki.bot.command",
            ConfigApplyRequest {
                candidate: ConfigValue::Object(
                    [(
                        "prefixes".into(),
                        ConfigValue::Array(vec![
                            ConfigValue::String("/".into()),
                            ConfigValue::String("!".into()),
                        ]),
                    )]
                    .into_iter()
                    .collect(),
                ),
                expected_revision: snap.revision,
                dry_run: false,
            },
            ctx,
            &caps,
        )
        .await
        .unwrap();
    assert!(result.actions.contains(&ConfigAction::PluginReloaded));
    assert!(
        !result
            .pending_actions
            .contains(&ConfigAction::PluginReloaded)
    );
    assert!(!calls.lock().unwrap().is_empty());
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
    let _ = ConfigRevision(1);
}
