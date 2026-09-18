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

pub fn attach_management_changed_bridges(
    host: &MutsukiWebHost,
    qq: Option<&Arc<dyn QqBotManagementApi>>,
    bilibili: Option<&Arc<dyn BilibiliManagementApi>>,
    sandbox: Option<&Arc<dyn SandboxApi>>,
) -> Option<ManagementChangeBridge> {
    let bridge = host.bridge()?.clone();
    let mut tasks = Vec::new();
    if let Some(mut changes) = qq.and_then(|api| api.subscribe_changes()) {
        let bridge = bridge.clone();
        tasks.push(tokio::spawn(async move {
            while let Some(event) = changes.changed().await {
                if let Ok(payload) = serde_json::to_value(event) {
                    let _ = bridge.publish_event("qq.changed", payload);
                }
            }
        }));
    }
    if let Some(mut changes) = bilibili.and_then(|api| api.subscribe_changes()) {
        let bridge = bridge.clone();
        tasks.push(tokio::spawn(async move {
            while let Some(event) = changes.changed().await {
                if let Ok(payload) = serde_json::to_value(event) {
                    let _ = bridge.publish_event("bilibili.changed", payload);
                }
            }
        }));
    }
    if let Some(mut changes) = sandbox.and_then(|api| api.subscribe_changes()) {
        tasks.push(tokio::spawn(async move {
            while let Some(event) = changes.changed().await {
                if let Ok(payload) = serde_json::to_value(event) {
                    let _ = bridge.publish_event("sandbox.changed", payload);
                }
            }
        }));
    }
    Some(ManagementChangeBridge { tasks })
}
