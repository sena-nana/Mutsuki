//! Read-only Host secret key presence for the embedded console (no value echo).

use std::sync::Arc;

use mutsuki_web_extension::{ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor};
use mutsuki_web_protocol::{EXTENSION_MANIFEST_VERSION, ExtensionManifest, WEB_PROTOCOL_VERSION};
use serde_json::{Value as JsonValue, json};

use mutsuki_plugin_bot_control_web::CAPABILITY_RUNTIME_READ;

pub const PLUGIN_ID: &str = "secret";
pub const PLUGIN_VERSION: &str = "0.1.0";

/// Resolve secret material for a Host key reference. Values must never leave this boundary.
pub trait SecretKeyResolver: Send + Sync {
    fn resolve(&self, key: &str) -> Option<String>;
}

#[derive(Clone)]
pub struct SecretMonitor {
    keys: Arc<[String]>,
    resolver: Arc<dyn SecretKeyResolver>,
}

impl SecretMonitor {
    pub fn new(keys: Vec<String>, resolver: Arc<dyn SecretKeyResolver>) -> Self {
        Self {
            keys: keys.into(),
            resolver,
        }
    }

    fn status_for(&self, key: &str) -> &'static str {
        match self.resolver.resolve(key) {
            None => "absent",
            Some(value) if value.trim().is_empty() => "invalid",
            Some(_) => "present",
        }
    }

    fn snapshot(&self) -> serde_json::Value {
        let secrets = self
            .keys
            .iter()
            .map(|key| {
                json!({
                    "key": key,
                    "state": self.status_for(key),
                })
            })
            .collect::<Vec<_>>();
        json!({ "secrets": secrets })
    }
}

pub struct SecretStatusWebExtension {
    monitor: SecretMonitor,
}

impl SecretStatusWebExtension {
    pub fn new(monitor: SecretMonitor) -> Self {
        Self { monitor }
    }
}

impl WebExtension for SecretStatusWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        ExtensionManifest {
            manifest_version: EXTENSION_MANIFEST_VERSION,
            id: PLUGIN_ID.into(),
            version: PLUGIN_VERSION.into(),
            entry: String::new(),
            capabilities: vec![CAPABILITY_RUNTIME_READ.into()],
            permissions: vec![],
            assets: vec![],
            protocol_version: WEB_PROTOCOL_VERSION.into(),
        }
    }

    fn frontend_assets(&self) -> Option<mutsuki_web_protocol::WebFrontendAssets> {
        None
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let monitor = self.monitor.clone();
        ctx.register("status", move |params| {
            require_runtime_read(&params)?;
            Ok(monitor.snapshot())
        });
        Ok(())
    }

    fn register_events(
        &self,
        _ctx: &mut mutsuki_web_extension::EventRegistry,
    ) -> Result<(), ExtensionError> {
        Ok(())
    }
}

fn require_runtime_read(params: &JsonValue) -> Result<(), ExtensionError> {
    let caps = params
        .get("capabilities")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    if caps
        .iter()
        .any(|cap| *cap == "*" || *cap == CAPABILITY_RUNTIME_READ)
    {
        Ok(())
    } else {
        Err(ExtensionError::Registration(format!(
            "capability denied: {CAPABILITY_RUNTIME_READ}"
        )))
    }
}
