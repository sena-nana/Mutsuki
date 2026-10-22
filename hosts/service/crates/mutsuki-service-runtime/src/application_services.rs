//! Application-scoped Host services shared between configured components.
//!
//! A component publishes a service with `register_application_service` and every consumer
//! declares `require_application_service` before resolving it through a [`RunnerFactoryContext`].
//! Factories resolve lazily, after every component has been prepared, so selection order never
//! decides whether a dependency exists. Reconfiguring a provider stages its replacement and
//! co-reloads every declared consumer; a failed reload restores the previous directory.

#[allow(clippy::wildcard_imports)]
use super::*;

use std::sync::RwLock;

use mutsuki_runtime_sdk::HostServiceValue;

/// Component id used for registrations made outside a configured factory.
pub(crate) const BUILTIN_COMPONENT: &str = "builtin";

#[derive(Clone, Default)]
pub(crate) struct ApplicationServiceTable {
    services: BTreeMap<String, (String, HostServiceValue)>,
    requirements: BTreeMap<String, BTreeSet<String>>,
}

impl ApplicationServiceTable {
    pub(crate) fn from_components(
        components: &PreparedComponentRegistry,
    ) -> ServiceRuntimeResult<Self> {
        let mut table = Self::default();
        for (component_id, component) in &components.records {
            for (service_id, value) in &component.application_services {
                if let Some((owner, _)) = table.services.get(service_id) {
                    return Err(ServiceRuntimeError::DuplicateApplicationService {
                        service_id: service_id.clone(),
                        first_owner: owner.clone(),
                        second_owner: component_id.clone(),
                    });
                }
                table
                    .services
                    .insert(service_id.clone(), (component_id.clone(), value.clone()));
            }
            if !component.required_application_services.is_empty() {
                table.requirements.insert(
                    component_id.clone(),
                    component.required_application_services.clone(),
                );
            }
        }
        for (component_id, required) in &table.requirements {
            if let Some(service_id) = required
                .iter()
                .find(|service_id| !table.services.contains_key(*service_id))
            {
                return Err(ServiceRuntimeError::MissingApplicationService {
                    component_id: component_id.clone(),
                    service_id: service_id.clone(),
                });
            }
        }
        Ok(table)
    }

    /// Components (other than `providers`) that declared a service one of `providers` owns.
    pub(crate) fn dependents_of(&self, providers: &BTreeSet<String>) -> BTreeSet<String> {
        let provided = self
            .services
            .iter()
            .filter(|(_, (owner, _))| providers.contains(owner))
            .map(|(service_id, _)| service_id.as_str())
            .collect::<BTreeSet<_>>();
        self.requirements
            .iter()
            .filter(|(component_id, required)| {
                !providers.contains(*component_id)
                    && required
                        .iter()
                        .any(|service_id| provided.contains(service_id.as_str()))
            })
            .map(|(component_id, _)| component_id.clone())
            .collect()
    }
}

/// Live directory of application services; cloned handles observe every committed generation.
#[derive(Clone, Default)]
pub struct ApplicationServiceDirectory {
    table: Arc<RwLock<Arc<ApplicationServiceTable>>>,
}

impl ApplicationServiceDirectory {
    pub(crate) fn snapshot(&self) -> Arc<ApplicationServiceTable> {
        self.table
            .read()
            .expect("application service directory lock")
            .clone()
    }

    /// Replaces the active table and returns the previous one for rollback.
    pub(crate) fn replace(&self, table: ApplicationServiceTable) -> Arc<ApplicationServiceTable> {
        std::mem::replace(
            &mut *self
                .table
                .write()
                .expect("application service directory lock"),
            Arc::new(table),
        )
    }

    pub(crate) fn restore(&self, table: Arc<ApplicationServiceTable>) {
        *self
            .table
            .write()
            .expect("application service directory lock") = table;
    }

    fn resolve<T: HostService>(
        &self,
        consumer: Option<&str>,
        service_id: &str,
    ) -> Result<Arc<T>, String> {
        let table = self.snapshot();
        if let Some(consumer) = consumer
            && !table
                .requirements
                .get(consumer)
                .is_some_and(|required| required.contains(service_id))
        {
            return Err(format!(
                "host_service.undeclared.{service_id}: component `{consumer}` must call \
                 require_application_service before resolving it"
            ));
        }
        let (_, value) = table
            .services
            .get(service_id)
            .ok_or_else(|| format!("host_service.unavailable.{service_id}"))?;
        value
            .resolve::<T>()
            .map_err(|failure| format!("{service_id}: {failure}"))
    }

    /// Resolves the currently committed provider without a consumer declaration (control plane).
    pub fn current<T: HostService>(&self, service_id: &str) -> Result<Arc<T>, String> {
        self.resolve(None, service_id)
    }
}

/// Services a scoped factory may use when it builds a runner, handler or loaded plugin.
///
/// Event sources and other long-lived owner objects may keep a clone and resolve at start time.
#[derive(Clone)]
pub struct RunnerFactoryContext {
    component_id: String,
    client: Arc<DeferredHostClient>,
    services: ApplicationServiceDirectory,
}

impl RunnerFactoryContext {
    pub(crate) fn new(
        component_id: String,
        client: Arc<DeferredHostClient>,
        services: ApplicationServiceDirectory,
    ) -> Self {
        Self {
            component_id,
            client,
            services,
        }
    }

    pub fn component_id(&self) -> &str {
        &self.component_id
    }

    /// Nested-task client; bound to the booted runtime before any runner executes.
    pub fn client(&self) -> RuntimeClientRef {
        self.client.clone()
    }

    pub fn resources(&self) -> Arc<dyn ResourceRegistryGateway> {
        self.client.clone()
    }

    /// Resolves a declared application service from the generation being prepared.
    pub fn require_service<T: HostService>(&self, service_id: &str) -> Result<Arc<T>, String> {
        self.services.resolve(Some(&self.component_id), service_id)
    }
}
