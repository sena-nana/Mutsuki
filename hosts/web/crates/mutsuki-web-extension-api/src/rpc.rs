use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use mutsuki_web_protocol::JsonValue;

use crate::ExtensionError;

#[derive(Clone, Debug, Default)]
pub struct RpcCallContext {
    principal_id: Arc<str>,
    capabilities: Arc<[String]>,
}

impl RpcCallContext {
    #[must_use]
    pub fn new(capabilities: &[String]) -> Self {
        Self::authenticated("internal", capabilities)
    }

    #[must_use]
    pub fn authenticated(principal_id: &str, capabilities: &[String]) -> Self {
        Self {
            principal_id: Arc::from(principal_id),
            capabilities: capabilities.to_vec().into(),
        }
    }

    #[must_use]
    pub fn principal_id(&self) -> &str {
        &self.principal_id
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

    pub fn register<F>(&mut self, method: &str, handler: F)
    where
        F: Fn(JsonValue) -> Result<JsonValue, ExtensionError> + Send + Sync + 'static,
    {
        let key = format!("{}.{}", self.namespace, method);
        self.handlers.insert(
            key,
            RegisteredRpcHandler::Sync(Arc::new(move |_context, params| handler(params))),
        );
    }

    pub fn register_contextual<F>(&mut self, method: &str, handler: F)
    where
        F: Fn(RpcCallContext, JsonValue) -> Result<JsonValue, ExtensionError>
            + Send
            + Sync
            + 'static,
    {
        let key = format!("{}.{}", self.namespace, method);
        self.handlers
            .insert(key, RegisteredRpcHandler::Sync(Arc::new(handler)));
    }

    pub fn register_async<F, Fut>(&mut self, method: &str, handler: F)
    where
        F: Fn(JsonValue) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<JsonValue, ExtensionError>> + Send + 'static,
    {
        let key = format!("{}.{}", self.namespace, method);
        self.handlers.insert(
            key,
            RegisteredRpcHandler::Async(Arc::new(move |_context, params| {
                Box::pin(handler(params))
            })),
        );
    }

    pub fn register_async_contextual<F, Fut>(&mut self, method: &str, handler: F)
    where
        F: Fn(RpcCallContext, JsonValue) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<JsonValue, ExtensionError>> + Send + 'static,
    {
        let key = format!("{}.{}", self.namespace, method);
        self.handlers.insert(
            key,
            RegisteredRpcHandler::Async(Arc::new(move |context, params| {
                Box::pin(handler(context, params))
            })),
        );
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
            RegisteredRpcHandler::Sync(handler) => {
                dispatch_sync_handler(handler, RpcCallContext::default(), params).await
            }
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
            RegisteredRpcHandler::Sync(handler) => {
                dispatch_sync_handler(handler, context, params).await
            }
            RegisteredRpcHandler::Async(handler) => handler(context, params).await,
        }
    }

    pub fn methods(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    pub fn is_async_method(&self, method: &str) -> bool {
        let key = if method.starts_with(&format!("{}.", self.namespace)) {
            method.to_owned()
        } else {
            format!("{}.{}", self.namespace, method)
        };
        matches!(
            self.handlers.get(&key),
            Some(RegisteredRpcHandler::Async(_))
        )
    }
}

async fn dispatch_sync_handler(
    handler: RpcHandler,
    context: RpcCallContext,
    params: JsonValue,
) -> Result<JsonValue, ExtensionError> {
    handler(context, params)
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

    pub fn register_topic(&mut self, topic: &str) {
        let full = format!("{}.{}", self.namespace, topic);
        self.topics.push(full);
    }

    pub fn topics(&self) -> &[String] {
        &self.topics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
