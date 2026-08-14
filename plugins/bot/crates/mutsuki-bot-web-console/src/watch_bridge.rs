//! Attach ConfigService revision_changed → WebBridge event fanout.

use std::sync::Arc;

use mutsuki_config_service::{ConfigService, ConfigWatchSubscription};
use mutsuki_web_host::MutsukiWebHost;
use serde_json::json;

pub fn attach_revision_changed_bridge(
    host: &MutsukiWebHost,
    service: &Arc<ConfigService>,
) -> Option<ConfigWatchSubscription> {
    let Some(bridge) = host.bridge().cloned() else {
        return None;
    };
    Some(service.subscribe_revision_changed(Arc::new(move |event| {
        let payload = json!({
            "provider_id": event.provider_id,
            "revision": event.revision,
            "context": event.context,
        });
        let _ = bridge.publish_event("config.revision_changed", payload);
    })))
}
