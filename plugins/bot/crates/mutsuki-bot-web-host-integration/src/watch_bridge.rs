//! Attach ConfigService revision_changed → WebBridge event fanout.

use std::sync::Arc;

use mutsuki_bot_management::{BilibiliManagementApi, QqBotManagementApi};
use mutsuki_bot_sandbox::SandboxApi;
use mutsuki_config_service::{ConfigService, ConfigWatchSubscription};
use mutsuki_service_control::ControlChangeSubscription;
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

pub struct ControlChangeBridge {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ControlChangeBridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn attach_control_changed_bridge(
    host: &MutsukiWebHost,
    mut changes: ControlChangeSubscription,
) -> Option<ControlChangeBridge> {
    let bridge = host.bridge()?.clone();
    let task = tokio::spawn(async move {
        while let Some(event) = changes.changed().await {
            let Ok(payload) = serde_json::to_value(event) else {
                continue;
            };
            let _ = bridge.publish_event("control.changed", payload);
        }
    });
    Some(ControlChangeBridge { task })
}

pub struct ManagementChangeBridge {
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for ManagementChangeBridge {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Resolves the Bilibili management service that is live right now.
pub type BilibiliResolver = Arc<dyn Fn() -> Option<Arc<dyn BilibiliManagementApi>> + Send + Sync>;

/// How long a bridge waits before re-subscribing after its owner went away (reload, disable).
const RESUBSCRIBE_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// Forwards owner change streams to `<owner>.changed` and `platforms.changed`.
///
/// A stream ends when its owner reloads or is disabled; each bridge then re-subscribes against
/// the owner that is live at that moment, so the console keeps receiving changes across reloads.
/// `qq` and `sandbox` must resolve their owner per call (the console bridges do).
pub fn attach_management_changed_bridges(
    host: &MutsukiWebHost,
    qq: Option<&Arc<dyn QqBotManagementApi>>,
    bilibili: Option<BilibiliResolver>,
    sandbox: Option<&Arc<dyn SandboxApi>>,
) -> Option<ManagementChangeBridge> {
    let bridge = host.bridge()?.clone();
    let mut tasks = Vec::new();
    if let Some(qq) = qq.cloned() {
        let bridge = bridge.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                if let Some(mut changes) = qq.subscribe_changes() {
                    while let Some(event) = changes.changed().await {
                        if let Ok(payload) = serde_json::to_value(event) {
                            let _ = bridge.publish_event("qq.changed", payload);
                            let _ = bridge.publish_event("platforms.changed", json!({}));
                        }
                    }
                }
                tokio::time::sleep(RESUBSCRIBE_DELAY).await;
            }
        }));
    }
    if let Some(resolve) = bilibili {
        let bridge = bridge.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                if let Some(mut changes) = resolve().and_then(|api| api.subscribe_changes()) {
                    while let Some(event) = changes.changed().await {
                        if let Ok(payload) = serde_json::to_value(event) {
                            let _ = bridge.publish_event("bilibili.changed", payload);
                            let _ = bridge.publish_event("platforms.changed", json!({}));
                        }
                    }
                }
                tokio::time::sleep(RESUBSCRIBE_DELAY).await;
            }
        }));
    }
    if let Some(sandbox) = sandbox.cloned() {
        tasks.push(tokio::spawn(async move {
            loop {
                if let Some(mut changes) = sandbox.subscribe_changes() {
                    while let Some(event) = changes.changed().await {
                        if let Ok(payload) = serde_json::to_value(event) {
                            let _ = bridge.publish_event("sandbox.changed", payload);
                        }
                    }
                }
                tokio::time::sleep(RESUBSCRIBE_DELAY).await;
            }
        }));
    }
    Some(ManagementChangeBridge { tasks })
}
