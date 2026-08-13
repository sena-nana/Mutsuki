//! Lifecycle integration with the real control surface.

use std::sync::Arc;

use mutsuki_bot::ControlPluginReloadLifecycle;
use mutsuki_config_service::{ConfigAction, ConfigLifecycle, ConfigValue, RestartPolicy};
use mutsuki_plugin_bot_control_web::FixtureControlHandler;

#[tokio::test]
async fn control_lifecycle_invokes_real_plugin_reload() {
    let fixture = Arc::new(FixtureControlHandler::default());
    let lifecycle = ControlPluginReloadLifecycle::new(fixture.clone(), "fixture");
    let completed = lifecycle
        .execute(
            "mutsuki.bot.command",
            &ConfigValue::Null,
            &ConfigValue::Null,
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
