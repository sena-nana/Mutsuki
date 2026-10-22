use std::collections::HashMap;
use std::sync::Arc;

use mutsuki_web_extension_api::{
    EventRegistry, ExtensionError, RpcCallContext, RpcRegistry, WebExtension,
};
use mutsuki_web_protocol::{
    ExtensionFailure, ExtensionManifest, JsonValue, ResourceBudgets, WebFrontendAssets,
};

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
    /// RPC namespace (see [`WebExtension::namespace`]).
    pub namespace: String,
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
    /// Keyed by extension id.
    records: HashMap<String, ExtensionRecord>,
    /// RPC namespace → extension id.
    namespaces: HashMap<String, String>,
    budgets: ResourceBudgets,
}

impl ExtensionRegistry {
    pub fn new(budgets: ResourceBudgets) -> Self {
        Self {
            records: HashMap::new(),
            namespaces: HashMap::new(),
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
        let namespace = extension.namespace();
        if let Some(owner) = self
            .namespaces
            .get(&namespace)
            .filter(|owner| **owner != manifest.id)
        {
            return Err(ExtensionError::Registration(format!(
                "namespace `{namespace}` of {} is already owned by {owner}",
                manifest.id
            )));
        }
        let mut rpc = RpcRegistry::new(namespace.clone());
        let mut events = EventRegistry::new(namespace.clone());
        extension.register_rpc(&mut rpc)?;
        extension.register_events(&mut events)?;

        let record = ExtensionRecord {
            manifest: manifest.clone(),
            namespace: namespace.clone(),
            assets: extension.frontend_assets(),
            rpc: Arc::new(rpc),
            events: Arc::new(events),
            enabled: true,
            failed: None,
        };
        self.namespaces.insert(namespace, manifest.id.clone());
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
                        styles: Vec::new(),
                        capabilities: vec![],
                        permissions: vec![],
                        assets: vec![],
                        protocol_version: mutsuki_web_protocol::WEB_PROTOCOL_VERSION.into(),
                    },
                    namespace: extension_id.to_string(),
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
            record.rpc = Arc::new(RpcRegistry::new(record.namespace.clone()));
            record.events = Arc::new(EventRegistry::new(record.namespace.clone()));
            true
        } else {
            false
        }
    }

    pub fn unload(&mut self, extension_id: &str) -> bool {
        let Some(record) = self.records.remove(extension_id) else {
            return false;
        };
        self.namespaces.remove(&record.namespace);
        true
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
        let record = self
            .namespaces
            .get(namespace)
            .and_then(|id| self.records.get(id))
            .ok_or_else(|| {
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
        let rpc = self.resolve_rpc(namespace, method, session_capabilities)?;
        let context = RpcCallContext::new(session_capabilities);
        if rpc.is_async_method(method) {
            return rpc.call_async_with_context(method, params, context).await;
        }
        let method = method.to_owned();
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => runtime
                .spawn_blocking(move || rpc.call_with_context(&method, params, context))
                .await
                .unwrap_or_else(|error| {
                    Err(ExtensionError::Rpc {
                        code: "rpc_failed".into(),
                        message: format!("synchronous rpc handler aborted: {error}"),
                    })
                }),
            Err(_) => rpc.call_with_context(&method, params, context),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_web_protocol::DEFAULT_BUDGETS;

    fn demo_record(rpc: RpcRegistry) -> ExtensionRecord {
        ExtensionRecord {
            manifest: ExtensionManifest {
                manifest_version: mutsuki_web_protocol::EXTENSION_MANIFEST_VERSION,
                id: "demo".into(),
                version: "1.0.0".into(),
                entry: "index.js".into(),
                styles: Vec::new(),
                capabilities: Vec::new(),
                permissions: Vec::new(),
                assets: Vec::new(),
                protocol_version: mutsuki_web_protocol::WEB_PROTOCOL_VERSION.into(),
            },
            namespace: "demo".into(),
            assets: None,
            rpc: Arc::new(rpc),
            events: Arc::new(EventRegistry::new("demo")),
            enabled: true,
            failed: None,
        }
    }

    /// The runtime has a single worker, so an inline synchronous handler would own it for the
    /// whole call and the release below could never be reached.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn synchronous_handlers_leave_the_async_worker_free() {
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let blocked = std::sync::Mutex::new(blocked);
        let mut rpc = RpcRegistry::new("demo");
        rpc.register("block", move |_params| {
            blocked
                .lock()
                .expect("release channel")
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|_| ExtensionError::Rpc {
                    code: "rpc_failed".into(),
                    message: "handler was never released".into(),
                })?;
            Ok(JsonValue::Null)
        });
        let mut registry = ExtensionRegistry::new(DEFAULT_BUDGETS);
        registry.records.insert("demo".into(), demo_record(rpc));
        registry.namespaces.insert("demo".into(), "demo".into());

        let call = tokio::spawn(async move {
            registry
                .call_rpc_async("demo", "block", JsonValue::Null, &[])
                .await
        });
        tokio::task::yield_now().await;
        release.send(()).expect("handler still waiting");

        call.await.expect("rpc task").expect("handler released");
    }

    #[test]
    fn disabling_extension_releases_owned_registrations() {
        let owned = Arc::new(());
        let weak = Arc::downgrade(&owned);
        let mut rpc = RpcRegistry::new("demo");
        rpc.register("ping", {
            let owned = owned.clone();
            move |_params| {
                let _ = &owned;
                Ok(JsonValue::Null)
            }
        });
        let mut registry = ExtensionRegistry::new(DEFAULT_BUDGETS);
        registry.records.insert("demo".into(), demo_record(rpc));
        registry.namespaces.insert("demo".into(), "demo".into());
        drop(owned);

        assert!(weak.upgrade().is_some());
        assert!(registry.disable("demo"));
        assert!(weak.upgrade().is_none());
    }

    struct Named {
        id: &'static str,
        namespace: &'static str,
    }

    impl WebExtension for Named {
        fn descriptor(&self) -> mutsuki_web_extension_api::WebExtensionDescriptor {
            ExtensionManifest {
                id: self.id.into(),
                ..demo_record(RpcRegistry::new("demo")).manifest
            }
        }

        fn namespace(&self) -> String {
            self.namespace.into()
        }

        fn frontend_assets(&self) -> Option<WebFrontendAssets> {
            None
        }

        fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
            ctx.register("ping", |_| Ok(JsonValue::String("pong".into())));
            Ok(())
        }

        fn register_events(&self, _ctx: &mut EventRegistry) -> Result<(), ExtensionError> {
            Ok(())
        }
    }

    #[test]
    fn qualified_ids_answer_on_their_short_namespace_and_namespaces_are_exclusive() {
        let mut registry = ExtensionRegistry::new(DEFAULT_BUDGETS);
        registry
            .load_extension(&Named {
                id: "mutsuki.web.diagnostics",
                namespace: "control",
            })
            .unwrap();
        assert_eq!(
            registry
                .call_rpc("control", "ping", JsonValue::Null, &[])
                .unwrap(),
            JsonValue::String("pong".into())
        );
        assert!(
            registry
                .call_rpc("mutsuki.web.diagnostics", "ping", JsonValue::Null, &[])
                .is_err()
        );
        assert!(registry.get("mutsuki.web.diagnostics").is_some());

        let Err(clash) = registry.load_extension(&Named {
            id: "mutsuki.other",
            namespace: "control",
        }) else {
            panic!("a second owner of `control` must be refused");
        };
        assert!(clash.to_string().contains("already owned"));

        assert!(registry.unload("mutsuki.web.diagnostics"));
        registry
            .load_extension(&Named {
                id: "mutsuki.other",
                namespace: "control",
            })
            .unwrap();
    }
}
