use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use mutsuki_web_protocol::{JsonValue, TypedRpc};

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

    /// Registers a [`TypedRpc`] method: the Host checks `M::CAPABILITY`, decodes the params
    /// (`invalid_params` on mismatch) and encodes the result, so handlers only see typed values.
    ///
    /// # Panics
    /// When `M::NAMESPACE` is not this registry's namespace (a programming error).
    pub fn register_typed<M, F, Fut>(&mut self, handler: F)
    where
        M: TypedRpc,
        F: Fn(RpcCallContext, M::Params) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<M::Result, ExtensionError>> + Send + 'static,
    {
        assert_eq!(
            M::NAMESPACE,
            self.namespace,
            "typed RPC {} registered on the wrong namespace",
            M::METHOD
        );
        let handler = Arc::new(handler);
        self.register_async_contextual(M::METHOD, move |context, params| {
            let handler = handler.clone();
            async move {
                context.require(M::CAPABILITY)?;
                let params = serde_json::from_value::<M::Params>(params).map_err(|error| {
                    ExtensionError::Rpc {
                        code: "invalid_params".into(),
                        message: format!("{}.{}: {error}", M::NAMESPACE, M::METHOD),
                    }
                })?;
                let result = handler(context, params).await?;
                serde_json::to_value(result).map_err(|error| ExtensionError::Rpc {
                    code: "invalid_result".into(),
                    message: format!("{}.{}: {error}", M::NAMESPACE, M::METHOD),
                })
            }
        });
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

#[cfg(test)]
mod typed_tests {
    use mutsuki_web_protocol::{RpcOperation, TypedRpc};
    use serde_json::json;

    use super::*;

    struct Double;

    impl TypedRpc for Double {
        const NAMESPACE: &'static str = "math";
        const METHOD: &'static str = "double";
        const OPERATION: RpcOperation = RpcOperation::Read;
        const CAPABILITY: &'static str = "math.read";
        const TS_PARAMS: &'static str = "number";
        const TS_RESULT: &'static str = "number";
        type Params = i64;
        type Result = i64;
    }

    fn registry() -> RpcRegistry {
        let mut registry = RpcRegistry::new("math");
        registry.register_typed::<Double, _, _>(|_context, value| async move { Ok(value * 2) });
        registry
    }

    #[tokio::test]
    async fn typed_rpc_checks_capability_decodes_params_and_encodes_result() {
        let registry = registry();
        let allowed = RpcCallContext::new(&["math.read".into()]);
        assert_eq!(
            registry
                .call_async_with_context("math.double", json!(21), allowed.clone())
                .await
                .unwrap(),
            json!(42)
        );
        let invalid = registry
            .call_async_with_context("math.double", json!("x"), allowed)
            .await
            .unwrap_err();
        assert_eq!(invalid.rpc_code(), "invalid_params");
        let denied = registry
            .call_async_with_context("math.double", json!(1), RpcCallContext::new(&[]))
            .await
            .unwrap_err();
        assert_eq!(denied.rpc_code(), "capability_denied");
    }
}
