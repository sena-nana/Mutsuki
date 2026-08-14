use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{HostService, HostServiceValue, RuntimeBootstrapperEffect};

use crate::error::host_failure;

const MAX_SCOPE_EVENTS: usize = 4096;

pub type EffectFuture<'a> = Pin<Box<dyn Future<Output = RuntimeResult<()>> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScopeId(Arc<str>);

impl ScopeId {
    #[must_use]
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(Arc::from(value.into()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ScopeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeState {
    Created,
    Resolving,
    Activating,
    Active,
    Suspending,
    Suspended,
    Draining,
    Disposing,
    Disposed,
    FailedDirty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginLifetime {
    RequiredBuiltin,
    Application,
    DrainRequired,
    RestartRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceRequirement {
    Required,
    Optional,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceBinding {
    StaticAtActivation,
    Rebindable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceDependency {
    pub service_id: String,
    pub requirement: ServiceRequirement,
    pub binding: ServiceBinding,
}

impl ServiceDependency {
    #[must_use]
    pub fn required(service_id: impl Into<String>) -> Self {
        Self {
            service_id: service_id.into(),
            requirement: ServiceRequirement::Required,
            binding: ServiceBinding::StaticAtActivation,
        }
    }

    #[must_use]
    pub fn optional(service_id: impl Into<String>) -> Self {
        Self {
            service_id: service_id.into(),
            requirement: ServiceRequirement::Optional,
            binding: ServiceBinding::StaticAtActivation,
        }
    }

    #[must_use]
    pub fn rebindable(mut self) -> Self {
        self.binding = ServiceBinding::Rebindable;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectKind {
    HostLocal,
    BackendInstance,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EffectId(u64);

impl EffectId {
    #[must_use]
    pub fn get(&self) -> u64 {
        self.0
    }
}

pub trait ScopedEffect: Send {
    fn dispose(&mut self) -> EffectFuture<'_>;
}

impl ScopedEffect for RuntimeBootstrapperEffect {
    fn dispose(&mut self) -> EffectFuture<'_> {
        self.effect.dispose()
    }
}

pub struct AsyncEffect<F> {
    cleanup: F,
}

impl<F> AsyncEffect<F> {
    #[must_use]
    pub fn new(cleanup: F) -> Self {
        Self { cleanup }
    }
}

impl<F, Fut> ScopedEffect for AsyncEffect<F>
where
    F: FnMut() -> Fut + Send,
    Fut: Future<Output = RuntimeResult<()>> + Send + 'static,
{
    fn dispose(&mut self) -> EffectFuture<'_> {
        Box::pin((self.cleanup)())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeEventKind {
    Created,
    StateChanged {
        from: ScopeState,
        to: ScopeState,
    },
    EffectRegistered {
        effect_id: EffectId,
        kind: EffectKind,
    },
    EffectDisposed {
        effect_id: EffectId,
    },
    EffectFailed {
        effect_id: EffectId,
        detail: String,
    },
    ServiceProvided {
        service_id: String,
    },
    ServiceWithdrawn {
        service_id: String,
    },
    DependencyUnavailable {
        service_id: String,
    },
    DependencyAvailable {
        service_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeEvent {
    pub sequence: u64,
    pub scope_id: ScopeId,
    pub kind: ScopeEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeSnapshot {
    pub scope_id: ScopeId,
    pub parent: Option<ScopeId>,
    pub plugin_id: String,
    pub plugin_generation: u64,
    pub state: ScopeState,
    pub lifetime: PluginLifetime,
    pub children: Vec<ScopeId>,
    pub dependencies: Vec<ServiceDependency>,
    pub provided_services: Vec<String>,
    pub effects: Vec<EffectSnapshot>,
    pub dirty_failures: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectSnapshot {
    pub effect_id: EffectId,
    pub kind: EffectKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisposeReport {
    pub disposed_scopes: Vec<ScopeId>,
    pub disposed_effects: Vec<EffectId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceChange {
    pub suspended: Vec<ScopeId>,
    pub ready_to_reactivate: Vec<ScopeId>,
}

pub struct ServiceKey<T> {
    id: &'static str,
    marker: PhantomData<fn() -> T>,
}

impl<T> Copy for ServiceKey<T> {}

impl<T> Clone for ServiceKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> ServiceKey<T> {
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self {
            id,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn id(self) -> &'static str {
        self.id
    }
}

#[derive(Clone)]
pub struct ResolvedService<T> {
    pub provider_scope: ScopeId,
    pub plugin_generation: u64,
    pub service: Arc<T>,
}

type ResolvedServiceValue = (ScopeId, u64, HostServiceValue);

struct ServiceEntry {
    provider_scope: ScopeId,
    generation: u64,
    rebindable: bool,
    available: bool,
    value: HostServiceValue,
}

#[derive(Clone)]
struct ServicePublication {
    visibility_scope: ScopeId,
    rebindable: bool,
}

struct EffectRecord {
    kind: EffectKind,
    effect: Box<dyn ScopedEffect>,
}

struct ScopeRecord {
    parent: Option<ScopeId>,
    plugin_id: String,
    plugin_generation: u64,
    state: ScopeState,
    lifetime: PluginLifetime,
    children: BTreeSet<ScopeId>,
    dependencies: BTreeMap<String, ServiceDependency>,
    isolated_services: BTreeSet<String>,
    services: BTreeMap<String, ServiceEntry>,
    service_publications: BTreeMap<String, ServicePublication>,
    resolved_services: BTreeMap<String, ScopeId>,
    effects: BTreeMap<EffectId, EffectRecord>,
    effect_order: Vec<EffectId>,
    dirty_failures: Vec<String>,
}

#[derive(Default)]
struct ScopeManagerState {
    scopes: BTreeMap<ScopeId, ScopeRecord>,
    next_scope_id: u64,
    next_effect_id: u64,
    next_event_sequence: u64,
    events: VecDeque<ScopeEvent>,
}

#[derive(Clone, Default)]
pub struct PluginScopeManager {
    inner: Arc<Mutex<ScopeManagerState>>,
}

impl std::fmt::Debug for PluginScopeManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("PluginScopeManager")
            .field(&Arc::as_ptr(&self.inner))
            .finish()
    }
}

impl PartialEq for PluginScopeManager {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for PluginScopeManager {}

#[derive(Clone, Copy)]
enum CleanupMode {
    Rollback,
    Dispose,
    Shutdown,
}

impl PluginScopeManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_scope(
        &self,
        parent: Option<&ScopeId>,
        plugin_id: impl Into<String>,
        plugin_generation: u64,
        lifetime: PluginLifetime,
    ) -> RuntimeResult<ScopeId> {
        let mut state = self.lock();
        let parent = parent.cloned();
        if let Some(parent_id) = &parent {
            let parent_record = state
                .scopes
                .get(parent_id)
                .ok_or_else(|| scope_failure("host.scope.parent_missing", parent_id.as_str()))?;
            if matches!(
                parent_record.state,
                ScopeState::Suspending
                    | ScopeState::Suspended
                    | ScopeState::Draining
                    | ScopeState::Disposing
                    | ScopeState::Disposed
                    | ScopeState::FailedDirty
            ) {
                return Err(scope_failure(
                    "host.scope.parent_inactive",
                    parent_id.as_str(),
                ));
            }
        }
        state.next_scope_id = state.next_scope_id.saturating_add(1);
        let scope_id = ScopeId::new(format!("scope:{}", state.next_scope_id));
        state.scopes.insert(
            scope_id.clone(),
            ScopeRecord {
                parent: parent.clone(),
                plugin_id: plugin_id.into(),
                plugin_generation,
                state: ScopeState::Created,
                lifetime,
                children: BTreeSet::new(),
                dependencies: BTreeMap::new(),
                isolated_services: BTreeSet::new(),
                services: BTreeMap::new(),
                service_publications: BTreeMap::new(),
                resolved_services: BTreeMap::new(),
                effects: BTreeMap::new(),
                effect_order: Vec::new(),
                dirty_failures: Vec::new(),
            },
        );
        if let Some(parent_id) = parent {
            state
                .scopes
                .get_mut(&parent_id)
                .expect("validated parent scope")
                .children
                .insert(scope_id.clone());
        }
        push_event(&mut state, &scope_id, ScopeEventKind::Created);
        Ok(scope_id)
    }

    pub fn declare_dependency(
        &self,
        scope_id: &ScopeId,
        dependency: ServiceDependency,
    ) -> RuntimeResult<()> {
        validate_service_id(&dependency.service_id)?;
        let mut state = self.lock();
        let scope = mutable_scope(&mut state, scope_id)?;
        require_configurable(scope, scope_id)?;
        if scope
            .dependencies
            .insert(dependency.service_id.clone(), dependency)
            .is_some()
        {
            return Err(scope_failure(
                "host.scope.dependency_duplicate",
                scope_id.as_str(),
            ));
        }
        Ok(())
    }

    pub fn isolate_service(
        &self,
        scope_id: &ScopeId,
        service_id: impl Into<String>,
    ) -> RuntimeResult<()> {
        let service_id = service_id.into();
        validate_service_id(&service_id)?;
        let mut state = self.lock();
        let scope = mutable_scope(&mut state, scope_id)?;
        require_configurable(scope, scope_id)?;
        scope.isolated_services.insert(service_id);
        Ok(())
    }

    pub fn begin_activation(&self, scope_id: &ScopeId) -> RuntimeResult<()> {
        let mut state = self.lock();
        let current = state
            .scopes
            .get(scope_id)
            .ok_or_else(|| scope_failure("host.scope.missing", scope_id.as_str()))?
            .state;
        if !matches!(current, ScopeState::Created | ScopeState::Suspended) {
            return Err(invalid_state(scope_id, current, "begin_activation"));
        }
        let resolved = resolve_dependencies(&state, scope_id)?;
        validate_required_dependency_cycles(&state, scope_id, &resolved)?;
        transition(&mut state, scope_id, &[current], ScopeState::Resolving)?;
        state
            .scopes
            .get_mut(scope_id)
            .expect("resolving scope exists")
            .resolved_services = resolved;
        transition(
            &mut state,
            scope_id,
            &[ScopeState::Resolving],
            ScopeState::Activating,
        )
    }

    pub fn commit_activation(&self, scope_id: &ScopeId) -> RuntimeResult<()> {
        let mut state = self.lock();
        let scope_state = state
            .scopes
            .get(scope_id)
            .ok_or_else(|| scope_failure("host.scope.missing", scope_id.as_str()))?
            .state;
        if scope_state != ScopeState::Activating {
            return Err(invalid_state(scope_id, scope_state, "commit_activation"));
        }
        let resolved = resolve_dependencies(&state, scope_id)?;
        validate_required_dependency_cycles(&state, scope_id, &resolved)?;
        state
            .scopes
            .get_mut(scope_id)
            .expect("activating scope exists")
            .resolved_services = resolved;
        for scope in state.scopes.values_mut() {
            for service in scope.services.values_mut() {
                if service.provider_scope == *scope_id {
                    service.available = true;
                }
            }
        }
        transition(
            &mut state,
            scope_id,
            &[ScopeState::Activating],
            ScopeState::Active,
        )
    }

    pub fn provide_service<T>(
        &self,
        scope_id: &ScopeId,
        key: ServiceKey<T>,
        service: Arc<T>,
        rebindable: bool,
    ) -> RuntimeResult<()>
    where
        T: HostService,
    {
        self.publish_service(scope_id, scope_id, key, service, rebindable)
    }

    pub fn publish_service<T>(
        &self,
        owner_scope: &ScopeId,
        visibility_scope: &ScopeId,
        key: ServiceKey<T>,
        service: Arc<T>,
        rebindable: bool,
    ) -> RuntimeResult<()>
    where
        T: HostService,
    {
        self.insert_service_value(
            owner_scope,
            visibility_scope,
            key.id(),
            HostServiceValue::new(service),
            rebindable,
            ScopeState::Activating,
        )
    }

    pub fn stage_service<T>(
        &self,
        scope_id: &ScopeId,
        key: ServiceKey<T>,
        service: Arc<T>,
        rebindable: bool,
    ) -> RuntimeResult<()>
    where
        T: HostService,
    {
        self.stage_published_service(scope_id, scope_id, key, service, rebindable)
    }

    pub fn stage_published_service<T>(
        &self,
        owner_scope: &ScopeId,
        visibility_scope: &ScopeId,
        key: ServiceKey<T>,
        service: Arc<T>,
        rebindable: bool,
    ) -> RuntimeResult<()>
    where
        T: HostService,
    {
        self.insert_service_value(
            owner_scope,
            visibility_scope,
            key.id(),
            HostServiceValue::new(service),
            rebindable,
            ScopeState::Created,
        )
    }

    pub(crate) fn stage_published_service_value(
        &self,
        owner_scope: &ScopeId,
        visibility_scope: &ScopeId,
        service_id: &str,
        service: HostServiceValue,
        rebindable: bool,
    ) -> RuntimeResult<()> {
        self.insert_service_value(
            owner_scope,
            visibility_scope,
            service_id,
            service,
            rebindable,
            ScopeState::Created,
        )
    }

    fn insert_service_value(
        &self,
        owner_scope: &ScopeId,
        visibility_scope: &ScopeId,
        service_id: &str,
        service: HostServiceValue,
        rebindable: bool,
        required_state: ScopeState,
    ) -> RuntimeResult<()> {
        validate_service_id(service_id)?;
        let mut state = self.lock();
        let generation = {
            let owner = state
                .scopes
                .get(owner_scope)
                .ok_or_else(|| scope_failure("host.scope.missing", owner_scope.as_str()))?;
            if owner.state != required_state {
                return Err(invalid_state(owner_scope, owner.state, "publish_service"));
            }
            owner.plugin_generation
        };
        let visibility = state
            .scopes
            .get(visibility_scope)
            .ok_or_else(|| scope_failure("host.scope.missing", visibility_scope.as_str()))?;
        if !is_ancestor_or_same(&state, visibility_scope, owner_scope) {
            return Err(scope_failure(
                "host.scope.service_visibility_unrelated",
                visibility_scope.as_str(),
            ));
        }
        if matches!(
            visibility.state,
            ScopeState::Disposing | ScopeState::Disposed
        ) {
            return Err(invalid_state(
                visibility_scope,
                visibility.state,
                "publish_service",
            ));
        }
        if visibility.services.contains_key(service_id) {
            return Err(scope_failure("host.scope.service_duplicate", service_id));
        }
        state
            .scopes
            .get_mut(visibility_scope)
            .expect("validated visibility scope")
            .services
            .insert(
                service_id.to_string(),
                ServiceEntry {
                    provider_scope: owner_scope.clone(),
                    generation,
                    rebindable,
                    available: false,
                    value: service,
                },
            );
        state
            .scopes
            .get_mut(owner_scope)
            .expect("validated owner scope")
            .service_publications
            .insert(
                service_id.to_string(),
                ServicePublication {
                    visibility_scope: visibility_scope.clone(),
                    rebindable,
                },
            );
        invalidate_resolution_cache(&mut state, service_id);
        push_event(
            &mut state,
            owner_scope,
            ScopeEventKind::ServiceProvided {
                service_id: service_id.to_string(),
            },
        );
        Ok(())
    }

    pub fn resolve_service<T>(
        &self,
        scope_id: &ScopeId,
        key: ServiceKey<T>,
    ) -> RuntimeResult<Option<ResolvedService<T>>>
    where
        T: HostService,
    {
        let Some((provider_scope, plugin_generation, service)) =
            self.resolve_service_value(scope_id, key.id())?
        else {
            return Ok(None);
        };
        let service = service
            .resolve::<T>()
            .map_err(|_| scope_failure("host.scope.service_type_mismatch", key.id()))?;
        Ok(Some(ResolvedService {
            provider_scope,
            plugin_generation,
            service,
        }))
    }

    pub(crate) fn resolve_service_value(
        &self,
        scope_id: &ScopeId,
        service_id: &str,
    ) -> RuntimeResult<Option<ResolvedServiceValue>> {
        let state = self.lock();
        let scope = state
            .scopes
            .get(scope_id)
            .ok_or_else(|| scope_failure("host.scope.missing", scope_id.as_str()))?;
        if !matches!(scope.state, ScopeState::Activating | ScopeState::Active) {
            return Err(invalid_state(scope_id, scope.state, "resolve_service"));
        }
        let provider_scope = if service_entry_for_provider(&state, scope_id, service_id).is_some() {
            Some(scope_id.clone())
        } else if scope.dependencies.contains_key(service_id) {
            scope.resolved_services.get(service_id).cloned()
        } else {
            return Err(scope_failure("host.scope.service_undeclared", service_id));
        };
        let Some(provider_scope) = provider_scope else {
            return Ok(None);
        };
        let entry = service_entry_for_provider(&state, &provider_scope, service_id)
            .ok_or_else(|| scope_failure("host.scope.service_stale", service_id))?;
        if scope.state == ScopeState::Active && !entry.available {
            return Err(scope_failure("host.scope.service_staged", service_id));
        }
        Ok(Some((
            provider_scope,
            entry.generation,
            entry.value.clone(),
        )))
    }

    pub fn register_effect(
        &self,
        scope_id: &ScopeId,
        kind: EffectKind,
        effect: Box<dyn ScopedEffect>,
    ) -> RuntimeResult<EffectId> {
        self.insert_effect(scope_id, kind, effect, ScopeState::Activating)
    }

    pub(crate) fn stage_effect(
        &self,
        scope_id: &ScopeId,
        kind: EffectKind,
        effect: Box<dyn ScopedEffect>,
    ) -> RuntimeResult<EffectId> {
        self.insert_effect(scope_id, kind, effect, ScopeState::Created)
    }

    fn insert_effect(
        &self,
        scope_id: &ScopeId,
        kind: EffectKind,
        effect: Box<dyn ScopedEffect>,
        required_state: ScopeState,
    ) -> RuntimeResult<EffectId> {
        let mut state = self.lock();
        let scope_state = state
            .scopes
            .get(scope_id)
            .ok_or_else(|| scope_failure("host.scope.missing", scope_id.as_str()))?
            .state;
        if scope_state != required_state {
            return Err(invalid_state(scope_id, scope_state, "insert_effect"));
        }
        state.next_effect_id = state.next_effect_id.saturating_add(1);
        let effect_id = EffectId(state.next_effect_id);
        let scope = state
            .scopes
            .get_mut(scope_id)
            .expect("validated effect owner scope");
        scope.effect_order.push(effect_id.clone());
        scope
            .effects
            .insert(effect_id.clone(), EffectRecord { kind, effect });
        push_event(
            &mut state,
            scope_id,
            ScopeEventKind::EffectRegistered {
                effect_id: effect_id.clone(),
                kind,
            },
        );
        Ok(effect_id)
    }

    pub async fn rollback_activation(
        &self,
        scope_id: &ScopeId,
        timeout: Duration,
    ) -> RuntimeResult<DisposeReport> {
        {
            let state = self.lock();
            let scope = state
                .scopes
                .get(scope_id)
                .ok_or_else(|| scope_failure("host.scope.missing", scope_id.as_str()))?;
            if !matches!(
                scope.state,
                ScopeState::Created
                    | ScopeState::Resolving
                    | ScopeState::Activating
                    | ScopeState::Active
                    | ScopeState::FailedDirty
            ) {
                return Err(invalid_state(scope_id, scope.state, "rollback_activation"));
            }
        }
        self.dispose_scope_internal(scope_id, tokio::time::Instant::now() + timeout, true, false)
            .await
    }

    pub fn rollback_activation_blocking(
        &self,
        scope_id: &ScopeId,
        timeout: Duration,
    ) -> RuntimeResult<DisposeReport> {
        self.run_cleanup_blocking_with_mode(scope_id, timeout, CleanupMode::Rollback)
    }

    pub async fn suspend_scope(
        &self,
        scope_id: &ScopeId,
        timeout: Duration,
    ) -> RuntimeResult<DisposeReport> {
        {
            let mut state = self.lock();
            transition(
                &mut state,
                scope_id,
                &[ScopeState::Active],
                ScopeState::Suspending,
            )?;
        }
        let report = self
            .dispose_effects_until(scope_id, tokio::time::Instant::now() + timeout)
            .await?;
        let mut state = self.lock();
        let scope = mutable_scope(&mut state, scope_id)?;
        scope.resolved_services.clear();
        transition(
            &mut state,
            scope_id,
            &[ScopeState::Suspending],
            ScopeState::Suspended,
        )?;
        Ok(report)
    }

    pub fn begin_drain(&self, scope_id: &ScopeId) -> RuntimeResult<()> {
        let mut state = self.lock();
        transition(
            &mut state,
            scope_id,
            &[ScopeState::Active, ScopeState::Suspended],
            ScopeState::Draining,
        )
    }

    pub async fn dispose_scope(
        &self,
        scope_id: &ScopeId,
        timeout: Duration,
    ) -> RuntimeResult<DisposeReport> {
        self.dispose_scope_internal(
            scope_id,
            tokio::time::Instant::now() + timeout,
            false,
            false,
        )
        .await
    }

    pub async fn shutdown_scope(
        &self,
        scope_id: &ScopeId,
        timeout: Duration,
    ) -> RuntimeResult<DisposeReport> {
        self.dispose_scope_internal(scope_id, tokio::time::Instant::now() + timeout, false, true)
            .await
    }

    pub fn dispose_scope_blocking(
        &self,
        scope_id: &ScopeId,
        timeout: Duration,
    ) -> RuntimeResult<DisposeReport> {
        self.run_cleanup_blocking_with_mode(scope_id, timeout, CleanupMode::Dispose)
    }

    pub fn shutdown_scope_blocking(
        &self,
        scope_id: &ScopeId,
        timeout: Duration,
    ) -> RuntimeResult<DisposeReport> {
        self.run_cleanup_blocking_with_mode(scope_id, timeout, CleanupMode::Shutdown)
    }

    fn run_cleanup_blocking_with_mode(
        &self,
        scope_id: &ScopeId,
        timeout: Duration,
        mode: CleanupMode,
    ) -> RuntimeResult<DisposeReport> {
        let manager = self.clone();
        let scope_id = scope_id.clone();
        std::thread::Builder::new()
            .name("mutsuki-scope-cleanup".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .map_err(|error| {
                        scope_failure("host.scope.cleanup_runtime", &error.to_string())
                    })?;
                runtime.block_on(async move {
                    match mode {
                        CleanupMode::Rollback => {
                            manager.rollback_activation(&scope_id, timeout).await
                        }
                        CleanupMode::Dispose => manager.dispose_scope(&scope_id, timeout).await,
                        CleanupMode::Shutdown => manager.shutdown_scope(&scope_id, timeout).await,
                    }
                })
            })
            .map_err(|error| scope_failure("host.scope.cleanup_thread", &error.to_string()))?
            .join()
            .map_err(|_| scope_failure("host.scope.cleanup_thread", "cleanup thread panicked"))?
    }

    fn dispose_scope_internal<'a>(
        &'a self,
        scope_id: &'a ScopeId,
        deadline: tokio::time::Instant,
        rollback: bool,
        shutdown: bool,
    ) -> Pin<Box<dyn Future<Output = RuntimeResult<DisposeReport>> + Send + 'a>> {
        Box::pin(async move {
            let children = {
                let mut state = self.lock();
                let Some(scope) = state.scopes.get(scope_id) else {
                    return if is_retired_scope_id(&state, scope_id) {
                        Ok(DisposeReport::default())
                    } else {
                        Err(scope_failure("host.scope.missing", scope_id.as_str()))
                    };
                };
                if scope.state == ScopeState::Disposed {
                    return Ok(DisposeReport::default());
                }
                if !rollback
                    && !shutdown
                    && matches!(
                        scope.lifetime,
                        PluginLifetime::RequiredBuiltin
                            | PluginLifetime::Application
                            | PluginLifetime::RestartRequired
                    )
                    && scope.state != ScopeState::FailedDirty
                {
                    return Err(scope_failure(
                        "host.scope.restart_required",
                        scope_id.as_str(),
                    ));
                }
                let allowed = if rollback {
                    vec![
                        ScopeState::Created,
                        ScopeState::Resolving,
                        ScopeState::Activating,
                        ScopeState::Active,
                        ScopeState::FailedDirty,
                    ]
                } else {
                    vec![
                        ScopeState::Created,
                        ScopeState::Active,
                        ScopeState::Suspended,
                        ScopeState::Draining,
                        ScopeState::FailedDirty,
                    ]
                };
                transition(&mut state, scope_id, &allowed, ScopeState::Disposing)?;
                state
                    .scopes
                    .get(scope_id)
                    .expect("disposing scope exists")
                    .children
                    .iter()
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>()
            };

            let mut report = DisposeReport::default();
            for child in children {
                let child_report = match self
                    .dispose_scope_internal(&child, deadline, rollback, shutdown)
                    .await
                {
                    Ok(report) => report,
                    Err(error) => {
                        let mut state = self.lock();
                        if let Some(scope) = state.scopes.get_mut(scope_id) {
                            scope
                                .dirty_failures
                                .push(format!("child scope {child} cleanup failed: {error}"));
                        }
                        mark_failed_dirty(&mut state, scope_id);
                        return Err(error);
                    }
                };
                report.disposed_scopes.extend(child_report.disposed_scopes);
                report
                    .disposed_effects
                    .extend(child_report.disposed_effects);
            }

            match self.dispose_effects_until(scope_id, deadline).await {
                Ok(effect_report) => report
                    .disposed_effects
                    .extend(effect_report.disposed_effects),
                Err(error) => return Err(error),
            }

            let mut state = self.lock();
            let (parent, services) = {
                let scope = mutable_scope(&mut state, scope_id)?;
                scope.resolved_services.clear();
                scope.dirty_failures.clear();
                (
                    scope.parent.clone(),
                    scope
                        .service_publications
                        .iter()
                        .map(|(service_id, publication)| {
                            (service_id.clone(), publication.visibility_scope.clone())
                        })
                        .collect::<Vec<_>>(),
                )
            };
            for (service_id, visibility_scope) in services {
                if let Some(visibility) = state.scopes.get_mut(&visibility_scope)
                    && visibility
                        .services
                        .get(&service_id)
                        .is_some_and(|service| service.provider_scope == *scope_id)
                {
                    visibility.services.remove(&service_id);
                }
                invalidate_resolution_cache(&mut state, &service_id);
                push_event(
                    &mut state,
                    scope_id,
                    ScopeEventKind::ServiceWithdrawn { service_id },
                );
            }
            transition(
                &mut state,
                scope_id,
                &[ScopeState::Disposing],
                ScopeState::Disposed,
            )?;
            if let Some(parent) = parent
                && let Some(parent) = state.scopes.get_mut(&parent)
            {
                parent.children.remove(scope_id);
            }
            report.disposed_scopes.push(scope_id.clone());
            state.scopes.remove(scope_id);
            Ok(report)
        })
    }

    async fn dispose_effects_until(
        &self,
        scope_id: &ScopeId,
        deadline: tokio::time::Instant,
    ) -> RuntimeResult<DisposeReport> {
        let order = {
            let state = self.lock();
            state
                .scopes
                .get(scope_id)
                .ok_or_else(|| scope_failure("host.scope.missing", scope_id.as_str()))?
                .effect_order
                .iter()
                .rev()
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut report = DisposeReport::default();
        for effect_id in order {
            let mut effect = {
                let mut state = self.lock();
                let Some(record) = state
                    .scopes
                    .get_mut(scope_id)
                    .and_then(|scope| scope.effects.remove(&effect_id))
                else {
                    continue;
                };
                record
            };
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let outcome = tokio::time::timeout(remaining, effect.effect.dispose()).await;
            match outcome {
                Ok(Ok(())) => {
                    let mut state = self.lock();
                    if let Some(scope) = state.scopes.get_mut(scope_id) {
                        scope.effect_order.retain(|known| known != &effect_id);
                    }
                    push_event(
                        &mut state,
                        scope_id,
                        ScopeEventKind::EffectDisposed {
                            effect_id: effect_id.clone(),
                        },
                    );
                    report.disposed_effects.push(effect_id);
                }
                Ok(Err(error)) => {
                    self.retain_dirty_effect(scope_id, effect_id, effect, error.to_string());
                    return Err(error);
                }
                Err(_) => {
                    let detail = "scope cleanup deadline exceeded".to_string();
                    self.retain_dirty_effect(scope_id, effect_id, effect, detail.clone());
                    return Err(scope_failure("host.scope.effect_timeout", &detail));
                }
            }
        }
        Ok(report)
    }

    fn retain_dirty_effect(
        &self,
        scope_id: &ScopeId,
        effect_id: EffectId,
        effect: EffectRecord,
        detail: String,
    ) {
        let mut state = self.lock();
        if let Some(scope) = state.scopes.get_mut(scope_id) {
            scope.effects.insert(effect_id.clone(), effect);
            scope.dirty_failures.push(detail.clone());
        }
        mark_failed_dirty(&mut state, scope_id);
        push_event(
            &mut state,
            scope_id,
            ScopeEventKind::EffectFailed { effect_id, detail },
        );
    }

    pub async fn withdraw_service(
        &self,
        provider_scope: &ScopeId,
        service_id: &str,
        timeout: Duration,
    ) -> RuntimeResult<ServiceChange> {
        let dependents = {
            let state = self.lock();
            let provider = state
                .scopes
                .get(provider_scope)
                .ok_or_else(|| scope_failure("host.scope.missing", provider_scope.as_str()))?;
            let publication = provider
                .service_publications
                .get(service_id)
                .ok_or_else(|| scope_failure("host.scope.service_missing", service_id))?;
            let service = state
                .scopes
                .get(&publication.visibility_scope)
                .and_then(|scope| scope.services.get(service_id))
                .filter(|service| service.provider_scope == *provider_scope)
                .ok_or_else(|| scope_failure("host.scope.service_missing", service_id))?;
            let mut dependents = Vec::new();
            for (scope_id, scope) in &state.scopes {
                let Some(dependency) = scope.dependencies.get(service_id) else {
                    continue;
                };
                if scope.resolved_services.get(service_id) != Some(provider_scope) {
                    continue;
                }
                if dependency.requirement == ServiceRequirement::Required
                    && (dependency.binding != ServiceBinding::Rebindable || !service.rebindable)
                    && matches!(scope.state, ScopeState::Active | ScopeState::Activating)
                {
                    return Err(scope_failure(
                        "host.scope.static_dependency_active",
                        scope_id.as_str(),
                    ));
                }
                dependents.push((scope_id.clone(), dependency.clone()));
            }
            dependents
        };

        {
            let mut state = self.lock();
            let visibility_scope = state
                .scopes
                .get(provider_scope)
                .expect("validated provider scope")
                .service_publications
                .get(service_id)
                .expect("validated service publication")
                .visibility_scope
                .clone();
            state
                .scopes
                .get_mut(&visibility_scope)
                .expect("validated service visibility scope")
                .services
                .remove(service_id);
            invalidate_resolution_cache(&mut state, service_id);
            push_event(
                &mut state,
                provider_scope,
                ScopeEventKind::ServiceWithdrawn {
                    service_id: service_id.to_string(),
                },
            );
            for (scope_id, _) in &dependents {
                push_event(
                    &mut state,
                    scope_id,
                    ScopeEventKind::DependencyUnavailable {
                        service_id: service_id.to_string(),
                    },
                );
            }
        }

        let mut change = ServiceChange::default();
        let mut failures = Vec::new();
        for (scope_id, dependency) in &dependents {
            if dependency.requirement == ServiceRequirement::Required
                && dependency.binding == ServiceBinding::Rebindable
            {
                if let Err(error) = self.suspend_scope(scope_id, timeout).await {
                    failures.push(format!("{scope_id}: {error}"));
                    continue;
                }
                change.suspended.push(scope_id.clone());
            }
        }
        if !failures.is_empty() {
            return Err(scope_failure(
                "host.scope.dependent_suspend_failed",
                &failures.join("; "),
            ));
        }
        Ok(change)
    }

    pub fn service_available(
        &self,
        provider_scope: &ScopeId,
        service_id: &str,
    ) -> RuntimeResult<ServiceChange> {
        let mut state = self.lock();
        let provider = state
            .scopes
            .get(provider_scope)
            .ok_or_else(|| scope_failure("host.scope.missing", provider_scope.as_str()))?;
        let publication = provider
            .service_publications
            .get(service_id)
            .ok_or_else(|| scope_failure("host.scope.service_missing", service_id))?;
        if !state
            .scopes
            .get(&publication.visibility_scope)
            .and_then(|scope| scope.services.get(service_id))
            .is_some_and(|service| service.available)
        {
            return Err(scope_failure("host.scope.service_missing", service_id));
        }
        invalidate_resolution_cache(&mut state, service_id);
        let candidates = state
            .scopes
            .iter()
            .filter_map(|(scope_id, scope)| {
                let dependency = scope.dependencies.get(service_id)?;
                (dependency.binding == ServiceBinding::Rebindable
                    && matches!(scope.state, ScopeState::Active | ScopeState::Suspended))
                .then(|| (scope_id.clone(), scope.state))
            })
            .collect::<Vec<_>>();
        let mut change = ServiceChange::default();
        for (scope_id, scope_state) in candidates {
            if let Ok(resolved) = resolve_dependencies(&state, &scope_id) {
                state
                    .scopes
                    .get_mut(&scope_id)
                    .expect("candidate scope exists")
                    .resolved_services = resolved;
                push_event(
                    &mut state,
                    &scope_id,
                    ScopeEventKind::DependencyAvailable {
                        service_id: service_id.to_string(),
                    },
                );
                if scope_state == ScopeState::Suspended {
                    change.ready_to_reactivate.push(scope_id);
                }
            }
        }
        Ok(change)
    }

    pub fn restore_service<T>(
        &self,
        provider_scope: &ScopeId,
        key: ServiceKey<T>,
        service: Arc<T>,
    ) -> RuntimeResult<ServiceChange>
    where
        T: HostService,
    {
        let service_id = key.id().to_string();
        let mut state = self.lock();
        let (generation, publication) = {
            let provider = mutable_scope(&mut state, provider_scope)?;
            if provider.state != ScopeState::Active {
                return Err(invalid_state(
                    provider_scope,
                    provider.state,
                    "restore_service",
                ));
            }
            let publication = provider
                .service_publications
                .get(&service_id)
                .cloned()
                .ok_or_else(|| scope_failure("host.scope.service_missing", &service_id))?;
            if !publication.rebindable {
                return Err(scope_failure(
                    "host.scope.service_not_rebindable",
                    &service_id,
                ));
            }
            (provider.plugin_generation, publication)
        };
        if state
            .scopes
            .get(&publication.visibility_scope)
            .and_then(|scope| scope.services.get(&service_id))
            .is_some()
        {
            return Err(scope_failure("host.scope.service_duplicate", &service_id));
        }
        state
            .scopes
            .get_mut(&publication.visibility_scope)
            .expect("validated service visibility scope")
            .services
            .insert(
                service_id.clone(),
                ServiceEntry {
                    provider_scope: provider_scope.clone(),
                    generation,
                    rebindable: true,
                    available: true,
                    value: HostServiceValue::new(service),
                },
            );
        invalidate_resolution_cache(&mut state, &service_id);
        push_event(
            &mut state,
            provider_scope,
            ScopeEventKind::ServiceProvided {
                service_id: service_id.clone(),
            },
        );
        drop(state);
        self.service_available(provider_scope, &service_id)
    }

    #[must_use]
    pub fn snapshot(&self, scope_id: &ScopeId) -> Option<ScopeSnapshot> {
        let state = self.lock();
        let scope = state.scopes.get(scope_id)?;
        Some(ScopeSnapshot {
            scope_id: scope_id.clone(),
            parent: scope.parent.clone(),
            plugin_id: scope.plugin_id.clone(),
            plugin_generation: scope.plugin_generation,
            state: scope.state,
            lifetime: scope.lifetime,
            children: scope.children.iter().cloned().collect(),
            dependencies: scope.dependencies.values().cloned().collect(),
            provided_services: scope.service_publications.keys().cloned().collect(),
            effects: effect_snapshots(scope),
            dirty_failures: scope.dirty_failures.clone(),
        })
    }

    #[must_use]
    pub fn snapshots(&self) -> Vec<ScopeSnapshot> {
        let state = self.lock();
        state
            .scopes
            .iter()
            .map(|(scope_id, scope)| ScopeSnapshot {
                scope_id: scope_id.clone(),
                parent: scope.parent.clone(),
                plugin_id: scope.plugin_id.clone(),
                plugin_generation: scope.plugin_generation,
                state: scope.state,
                lifetime: scope.lifetime,
                children: scope.children.iter().cloned().collect(),
                dependencies: scope.dependencies.values().cloned().collect(),
                provided_services: scope.service_publications.keys().cloned().collect(),
                effects: effect_snapshots(scope),
                dirty_failures: scope.dirty_failures.clone(),
            })
            .collect()
    }

    #[must_use]
    pub fn events_after(&self, sequence: u64) -> Vec<ScopeEvent> {
        self.lock()
            .events
            .iter()
            .filter(|event| event.sequence > sequence)
            .cloned()
            .collect()
    }

    pub(crate) fn mark_scope_failed_dirty(
        &self,
        scope_id: &ScopeId,
        detail: impl Into<String>,
    ) -> RuntimeResult<()> {
        let mut state = self.lock();
        mutable_scope(&mut state, scope_id)?
            .dirty_failures
            .push(detail.into());
        mark_failed_dirty(&mut state, scope_id);
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ScopeManagerState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn resolve_dependencies(
    state: &ScopeManagerState,
    scope_id: &ScopeId,
) -> RuntimeResult<BTreeMap<String, ScopeId>> {
    let scope = state
        .scopes
        .get(scope_id)
        .ok_or_else(|| scope_failure("host.scope.missing", scope_id.as_str()))?;
    let mut resolved = BTreeMap::new();
    for dependency in scope.dependencies.values() {
        match find_service_provider(state, scope_id, &dependency.service_id) {
            Some(provider) => {
                resolved.insert(dependency.service_id.clone(), provider);
            }
            None if dependency.requirement == ServiceRequirement::Optional => {}
            None => {
                return Err(scope_failure(
                    "host.scope.required_service_missing",
                    &dependency.service_id,
                ));
            }
        }
    }
    Ok(resolved)
}

fn effect_snapshots(scope: &ScopeRecord) -> Vec<EffectSnapshot> {
    scope
        .effect_order
        .iter()
        .filter_map(|effect_id| {
            scope.effects.get(effect_id).map(|effect| EffectSnapshot {
                effect_id: effect_id.clone(),
                kind: effect.kind,
            })
        })
        .collect()
}

fn find_service_provider(
    state: &ScopeManagerState,
    origin: &ScopeId,
    service_id: &str,
) -> Option<ScopeId> {
    let mut current = Some(origin.clone());
    while let Some(scope_id) = current {
        let scope = state.scopes.get(&scope_id)?;
        if scope.services.get(service_id).is_some_and(|service| {
            service.available
                || matches!(
                    state.scopes.get(origin).map(|scope| scope.state),
                    Some(ScopeState::Created | ScopeState::Resolving | ScopeState::Activating)
                )
        }) {
            return scope
                .services
                .get(service_id)
                .map(|service| service.provider_scope.clone());
        }
        if scope.isolated_services.contains(service_id) {
            return None;
        }
        current = scope.parent.clone();
    }
    None
}

fn service_entry_for_provider<'a>(
    state: &'a ScopeManagerState,
    provider_scope: &ScopeId,
    service_id: &str,
) -> Option<&'a ServiceEntry> {
    let publication = state
        .scopes
        .get(provider_scope)?
        .service_publications
        .get(service_id)?;
    state
        .scopes
        .get(&publication.visibility_scope)?
        .services
        .get(service_id)
        .filter(|entry| entry.provider_scope == *provider_scope)
}

fn is_ancestor_or_same(
    state: &ScopeManagerState,
    ancestor: &ScopeId,
    descendant: &ScopeId,
) -> bool {
    let mut current = Some(descendant.clone());
    while let Some(scope_id) = current {
        if scope_id == *ancestor {
            return true;
        }
        current = state
            .scopes
            .get(&scope_id)
            .and_then(|scope| scope.parent.clone());
    }
    false
}

fn validate_required_dependency_cycles(
    state: &ScopeManagerState,
    origin: &ScopeId,
    origin_resolved: &BTreeMap<String, ScopeId>,
) -> RuntimeResult<()> {
    fn visit(
        state: &ScopeManagerState,
        current: &ScopeId,
        origin: &ScopeId,
        origin_resolved: &BTreeMap<String, ScopeId>,
        visiting: &mut BTreeSet<ScopeId>,
        visited: &mut BTreeSet<ScopeId>,
    ) -> RuntimeResult<()> {
        if !visiting.insert(current.clone()) {
            return Err(scope_failure(
                "host.scope.required_dependency_cycle",
                current.as_str(),
            ));
        }
        if visited.contains(current) {
            visiting.remove(current);
            return Ok(());
        }
        let scope = state
            .scopes
            .get(current)
            .ok_or_else(|| scope_failure("host.scope.missing", current.as_str()))?;
        for dependency in scope
            .dependencies
            .values()
            .filter(|dependency| dependency.requirement == ServiceRequirement::Required)
        {
            let provider = if current == origin {
                origin_resolved.get(&dependency.service_id).cloned()
            } else {
                scope.resolved_services.get(&dependency.service_id).cloned()
            };
            if let Some(provider) = provider {
                visit(state, &provider, origin, origin_resolved, visiting, visited)?;
            }
        }
        visiting.remove(current);
        visited.insert(current.clone());
        Ok(())
    }

    visit(
        state,
        origin,
        origin,
        origin_resolved,
        &mut BTreeSet::new(),
        &mut BTreeSet::new(),
    )
}

fn transition(
    state: &mut ScopeManagerState,
    scope_id: &ScopeId,
    allowed: &[ScopeState],
    target: ScopeState,
) -> RuntimeResult<()> {
    let current = state
        .scopes
        .get(scope_id)
        .ok_or_else(|| scope_failure("host.scope.missing", scope_id.as_str()))?
        .state;
    if !allowed.contains(&current) {
        return Err(invalid_state(scope_id, current, "transition"));
    }
    state
        .scopes
        .get_mut(scope_id)
        .expect("validated scope transition")
        .state = target;
    push_event(
        state,
        scope_id,
        ScopeEventKind::StateChanged {
            from: current,
            to: target,
        },
    );
    Ok(())
}

fn mark_failed_dirty(state: &mut ScopeManagerState, scope_id: &ScopeId) {
    let Some(scope) = state.scopes.get_mut(scope_id) else {
        return;
    };
    let from = scope.state;
    if from == ScopeState::FailedDirty {
        return;
    }
    scope.state = ScopeState::FailedDirty;
    push_event(
        state,
        scope_id,
        ScopeEventKind::StateChanged {
            from,
            to: ScopeState::FailedDirty,
        },
    );
}

fn mutable_scope<'a>(
    state: &'a mut ScopeManagerState,
    scope_id: &ScopeId,
) -> RuntimeResult<&'a mut ScopeRecord> {
    state
        .scopes
        .get_mut(scope_id)
        .ok_or_else(|| scope_failure("host.scope.missing", scope_id.as_str()))
}

fn require_configurable(scope: &ScopeRecord, scope_id: &ScopeId) -> RuntimeResult<()> {
    if scope.state == ScopeState::Created {
        Ok(())
    } else {
        Err(invalid_state(scope_id, scope.state, "configure"))
    }
}

fn invalidate_resolution_cache(state: &mut ScopeManagerState, service_id: &str) {
    for scope in state.scopes.values_mut() {
        scope.resolved_services.remove(service_id);
    }
}

fn validate_service_id(service_id: &str) -> RuntimeResult<()> {
    if service_id.trim().is_empty() {
        Err(scope_failure("host.scope.service_id_empty", service_id))
    } else {
        Ok(())
    }
}

fn push_event(state: &mut ScopeManagerState, scope_id: &ScopeId, kind: ScopeEventKind) {
    state.next_event_sequence = state.next_event_sequence.saturating_add(1);
    state.events.push_back(ScopeEvent {
        sequence: state.next_event_sequence,
        scope_id: scope_id.clone(),
        kind,
    });
    while state.events.len() > MAX_SCOPE_EVENTS {
        state.events.pop_front();
    }
}

fn is_retired_scope_id(state: &ScopeManagerState, scope_id: &ScopeId) -> bool {
    scope_id
        .as_str()
        .strip_prefix("scope:")
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|value| value > 0 && value <= state.next_scope_id)
}

fn invalid_state(scope_id: &ScopeId, state: ScopeState, action: &str) -> RuntimeFailure {
    scope_failure(
        "host.scope.invalid_state",
        &format!("{scope_id}:{state:?}:{action}"),
    )
}

fn scope_failure(route: &'static str, detail: &str) -> RuntimeFailure {
    host_failure(route, detail)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    const TEXT: ServiceKey<String> = ServiceKey::new("test.text");

    fn active_scope(
        manager: &PluginScopeManager,
        parent: Option<&ScopeId>,
        plugin_id: &str,
    ) -> ScopeId {
        let scope = manager
            .create_scope(parent, plugin_id, 1, PluginLifetime::DrainRequired)
            .unwrap();
        manager.begin_activation(&scope).unwrap();
        manager.commit_activation(&scope).unwrap();
        scope
    }

    #[tokio::test]
    async fn dispose_is_child_first_reverse_order_and_idempotent() {
        let manager = PluginScopeManager::new();
        let order = Arc::new(Mutex::new(Vec::new()));
        let root = manager
            .create_scope(None, "root", 1, PluginLifetime::DrainRequired)
            .unwrap();
        manager.begin_activation(&root).unwrap();
        for name in ["root-a", "root-b"] {
            let order = order.clone();
            manager
                .register_effect(
                    &root,
                    EffectKind::HostLocal,
                    Box::new(AsyncEffect::new(move || {
                        let order = order.clone();
                        async move {
                            order.lock().unwrap().push(name);
                            Ok(())
                        }
                    })),
                )
                .unwrap();
        }
        manager.commit_activation(&root).unwrap();

        let child = manager
            .create_scope(Some(&root), "child", 1, PluginLifetime::DrainRequired)
            .unwrap();
        manager.begin_activation(&child).unwrap();
        let child_order = order.clone();
        manager
            .register_effect(
                &child,
                EffectKind::BackendInstance,
                Box::new(AsyncEffect::new(move || {
                    let order = child_order.clone();
                    async move {
                        order.lock().unwrap().push("child");
                        Ok(())
                    }
                })),
            )
            .unwrap();
        manager.commit_activation(&child).unwrap();

        let report = manager
            .dispose_scope(&root, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(*order.lock().unwrap(), ["child", "root-b", "root-a"]);
        assert_eq!(report.disposed_scopes, [child, root.clone()]);
        assert!(
            manager
                .dispose_scope(&root, Duration::from_secs(1))
                .await
                .unwrap()
                .disposed_scopes
                .is_empty()
        );
    }

    #[tokio::test]
    async fn failed_cleanup_retains_dirty_scope_and_can_be_retried() {
        let manager = PluginScopeManager::new();
        let first = Arc::new(AtomicBool::new(true));
        let scope = manager
            .create_scope(None, "dirty", 7, PluginLifetime::DrainRequired)
            .unwrap();
        manager.begin_activation(&scope).unwrap();
        let fail = first.clone();
        manager
            .register_effect(
                &scope,
                EffectKind::BackendInstance,
                Box::new(AsyncEffect::new(move || {
                    let should_fail = fail.swap(false, Ordering::AcqRel);
                    async move {
                        if should_fail {
                            Err(scope_failure("test.cleanup", "injected"))
                        } else {
                            Ok(())
                        }
                    }
                })),
            )
            .unwrap();
        manager.commit_activation(&scope).unwrap();

        assert!(
            manager
                .dispose_scope(&scope, Duration::from_secs(1))
                .await
                .is_err()
        );
        let dirty = manager.snapshot(&scope).unwrap();
        assert_eq!(dirty.state, ScopeState::FailedDirty);
        assert_eq!(dirty.effects.len(), 1);
        assert_eq!(dirty.effects[0].kind, EffectKind::BackendInstance);
        assert!(!dirty.dirty_failures.is_empty());
        assert!(manager.events_after(0).iter().any(|event| {
            event.scope_id == scope
                && matches!(
                    event.kind,
                    ScopeEventKind::StateChanged {
                        to: ScopeState::FailedDirty,
                        ..
                    }
                )
        }));

        manager
            .dispose_scope(&scope, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(manager.snapshot(&scope).is_none());
    }

    #[tokio::test]
    async fn activation_rollback_cleans_partial_effects() {
        let manager = PluginScopeManager::new();
        let cleaned = Arc::new(AtomicBool::new(false));
        let scope = manager
            .create_scope(None, "staged", 2, PluginLifetime::RestartRequired)
            .unwrap();
        manager.begin_activation(&scope).unwrap();
        let observed = cleaned.clone();
        manager
            .register_effect(
                &scope,
                EffectKind::HostLocal,
                Box::new(AsyncEffect::new(move || {
                    observed.store(true, Ordering::Release);
                    async { Ok(()) }
                })),
            )
            .unwrap();
        manager
            .rollback_activation(&scope, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(cleaned.load(Ordering::Acquire));
        assert!(manager.snapshot(&scope).is_none());
    }

    #[tokio::test]
    async fn activation_failure_after_each_registration_rolls_back_completed_effects() {
        for completed in 0..=3 {
            let manager = PluginScopeManager::new();
            let order = Arc::new(Mutex::new(Vec::new()));
            let scope = manager
                .create_scope(None, "staged", 2, PluginLifetime::DrainRequired)
                .unwrap();
            manager.begin_activation(&scope).unwrap();
            for index in 0..completed {
                let order = order.clone();
                manager
                    .register_effect(
                        &scope,
                        EffectKind::HostLocal,
                        Box::new(AsyncEffect::new(move || {
                            let order = order.clone();
                            async move {
                                order.lock().unwrap().push(index);
                                Ok(())
                            }
                        })),
                    )
                    .unwrap();
            }

            manager
                .rollback_activation(&scope, Duration::from_secs(1))
                .await
                .unwrap();

            assert_eq!(
                *order.lock().unwrap(),
                (0..completed).rev().collect::<Vec<_>>()
            );
            assert!(manager.snapshot(&scope).is_none());
        }
    }

    #[tokio::test]
    async fn failure_at_each_dispose_step_is_dirty_and_retryable_without_repeating_successes() {
        for failing_index in 0..3 {
            let manager = PluginScopeManager::new();
            let order = Arc::new(Mutex::new(Vec::new()));
            let scope = manager
                .create_scope(None, "dirty", 7, PluginLifetime::DrainRequired)
                .unwrap();
            manager.begin_activation(&scope).unwrap();
            for index in 0..3 {
                let order = order.clone();
                let fail_once = Arc::new(AtomicBool::new(index == failing_index));
                manager
                    .register_effect(
                        &scope,
                        EffectKind::BackendInstance,
                        Box::new(AsyncEffect::new(move || {
                            let order = order.clone();
                            let fail = fail_once.swap(false, Ordering::AcqRel);
                            async move {
                                order.lock().unwrap().push(index);
                                if fail {
                                    Err(scope_failure("test.cleanup", "injected"))
                                } else {
                                    Ok(())
                                }
                            }
                        })),
                    )
                    .unwrap();
            }
            manager.commit_activation(&scope).unwrap();

            assert!(
                manager
                    .dispose_scope(&scope, Duration::from_secs(1))
                    .await
                    .is_err()
            );
            assert_eq!(
                manager.snapshot(&scope).unwrap().state,
                ScopeState::FailedDirty
            );
            manager
                .dispose_scope(&scope, Duration::from_secs(1))
                .await
                .unwrap();

            let mut expected = (failing_index..3).rev().collect::<Vec<_>>();
            expected.extend((0..=failing_index).rev());
            assert_eq!(*order.lock().unwrap(), expected);
            assert!(manager.snapshot(&scope).is_none());
        }
    }

    #[tokio::test]
    async fn cleanup_timeout_is_one_scope_deadline() {
        let manager = PluginScopeManager::new();
        let scope = manager
            .create_scope(None, "deadline", 1, PluginLifetime::DrainRequired)
            .unwrap();
        manager.begin_activation(&scope).unwrap();
        for _ in 0..2 {
            manager
                .register_effect(
                    &scope,
                    EffectKind::HostLocal,
                    Box::new(AsyncEffect::new(|| async {
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        Ok(())
                    })),
                )
                .unwrap();
        }
        manager.commit_activation(&scope).unwrap();

        assert!(
            manager
                .dispose_scope(&scope, Duration::from_millis(45))
                .await
                .is_err()
        );
        let dirty = manager.snapshot(&scope).unwrap();
        assert_eq!(dirty.state, ScopeState::FailedDirty);
        assert_eq!(dirty.effects.len(), 1);
    }

    #[test]
    fn required_cycle_fails_before_activation() {
        let manager = PluginScopeManager::new();
        let scope = manager
            .create_scope(None, "cycle", 1, PluginLifetime::DrainRequired)
            .unwrap();
        manager
            .declare_dependency(&scope, ServiceDependency::required(TEXT.id()))
            .unwrap();
        manager
            .stage_service(&scope, TEXT, Arc::new("self".to_string()), false)
            .unwrap();
        assert!(manager.begin_activation(&scope).is_err());
        assert_eq!(manager.snapshot(&scope).unwrap().state, ScopeState::Created);
    }

    #[test]
    fn parent_lookup_optional_dependency_and_isolation_are_explicit() {
        let manager = PluginScopeManager::new();
        let root = manager
            .create_scope(None, "root", 4, PluginLifetime::DrainRequired)
            .unwrap();
        manager.begin_activation(&root).unwrap();
        manager
            .provide_service(&root, TEXT, Arc::new("root".to_string()), true)
            .unwrap();
        manager.commit_activation(&root).unwrap();

        let child = manager
            .create_scope(Some(&root), "child", 8, PluginLifetime::DrainRequired)
            .unwrap();
        manager
            .declare_dependency(&child, ServiceDependency::required(TEXT.id()))
            .unwrap();
        manager
            .declare_dependency(&child, ServiceDependency::optional("test.absent"))
            .unwrap();
        manager.begin_activation(&child).unwrap();
        let resolved = manager.resolve_service(&child, TEXT).unwrap().unwrap();
        assert_eq!(resolved.provider_scope, root);
        assert_eq!(resolved.plugin_generation, 4);
        assert_eq!(resolved.service.as_str(), "root");
        manager.commit_activation(&child).unwrap();

        let isolated = manager
            .create_scope(Some(&root), "isolated", 9, PluginLifetime::DrainRequired)
            .unwrap();
        manager
            .declare_dependency(&isolated, ServiceDependency::required(TEXT.id()))
            .unwrap();
        manager.isolate_service(&isolated, TEXT.id()).unwrap();
        assert!(manager.begin_activation(&isolated).is_err());
    }

    #[test]
    fn service_publication_rejects_unrelated_visibility_scope() {
        let manager = PluginScopeManager::new();
        let owner = manager
            .create_scope(None, "owner", 1, PluginLifetime::DrainRequired)
            .unwrap();
        let unrelated = manager
            .create_scope(None, "unrelated", 1, PluginLifetime::DrainRequired)
            .unwrap();

        assert!(
            manager
                .stage_published_service(
                    &owner,
                    &unrelated,
                    TEXT,
                    Arc::new("hidden".to_string()),
                    false,
                )
                .is_err()
        );
        assert!(
            manager
                .snapshot(&owner)
                .unwrap()
                .provided_services
                .is_empty()
        );
    }

    #[tokio::test]
    async fn rebindable_service_loss_suspends_and_recovery_requests_reactivation() {
        let manager = PluginScopeManager::new();
        let root = manager
            .create_scope(None, "provider", 1, PluginLifetime::DrainRequired)
            .unwrap();
        manager.begin_activation(&root).unwrap();
        manager
            .provide_service(&root, TEXT, Arc::new("v1".to_string()), true)
            .unwrap();
        manager.commit_activation(&root).unwrap();

        let child = manager
            .create_scope(Some(&root), "consumer", 1, PluginLifetime::DrainRequired)
            .unwrap();
        manager
            .declare_dependency(&child, ServiceDependency::required(TEXT.id()).rebindable())
            .unwrap();
        manager.begin_activation(&child).unwrap();
        manager.commit_activation(&child).unwrap();

        let change = manager
            .withdraw_service(&root, TEXT.id(), Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(change.suspended.as_slice(), std::slice::from_ref(&child));
        assert_eq!(
            manager.snapshot(&child).unwrap().state,
            ScopeState::Suspended
        );

        let change = manager
            .restore_service(&root, TEXT, Arc::new("v2".to_string()))
            .unwrap();
        assert_eq!(
            change.ready_to_reactivate.as_slice(),
            std::slice::from_ref(&child)
        );
        manager.begin_activation(&child).unwrap();
        manager.commit_activation(&child).unwrap();
        assert_eq!(
            manager
                .resolve_service(&child, TEXT)
                .unwrap()
                .unwrap()
                .service
                .as_str(),
            "v2"
        );
    }

    #[test]
    fn helper_builds_active_scope() {
        let manager = PluginScopeManager::new();
        let scope = active_scope(&manager, None, "helper");
        assert_eq!(manager.snapshot(&scope).unwrap().state, ScopeState::Active);
    }

    #[test]
    fn invalid_activation_commit_is_side_effect_free() {
        let manager = PluginScopeManager::new();
        let scope = manager
            .create_scope(None, "created", 1, PluginLifetime::DrainRequired)
            .unwrap();

        assert!(manager.commit_activation(&scope).is_err());
        assert_eq!(manager.snapshot(&scope).unwrap().state, ScopeState::Created);
        assert!(!manager.events_after(0).iter().any(|event| {
            matches!(
                event.kind,
                ScopeEventKind::StateChanged {
                    to: ScopeState::Active,
                    ..
                }
            )
        }));
    }

    #[test]
    fn inactive_parent_rejects_new_children() {
        let manager = PluginScopeManager::new();
        let parent = active_scope(&manager, None, "parent");
        manager.begin_drain(&parent).unwrap();

        assert!(
            manager
                .create_scope(
                    Some(&parent),
                    "late-child",
                    1,
                    PluginLifetime::DrainRequired,
                )
                .is_err()
        );
        assert!(manager.snapshot(&parent).unwrap().children.is_empty());
    }

    #[tokio::test]
    async fn application_lifetime_requires_shutdown_path() {
        let manager = PluginScopeManager::new();
        let scope = manager
            .create_scope(None, "application", 1, PluginLifetime::Application)
            .unwrap();
        manager.begin_activation(&scope).unwrap();
        manager.commit_activation(&scope).unwrap();
        assert!(
            manager
                .dispose_scope(&scope, Duration::from_secs(1))
                .await
                .is_err()
        );
        manager
            .shutdown_scope(&scope, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(manager.snapshot(&scope).is_none());
    }

    #[tokio::test]
    async fn repeated_scope_lifecycle_keeps_records_and_diagnostics_bounded() {
        let manager = PluginScopeManager::new();
        for generation in 1..=10_000 {
            let scope = manager
                .create_scope(None, "loop", generation, PluginLifetime::DrainRequired)
                .unwrap();
            manager.begin_activation(&scope).unwrap();
            manager.commit_activation(&scope).unwrap();
            manager
                .dispose_scope(&scope, Duration::from_secs(1))
                .await
                .unwrap();
        }
        assert!(manager.snapshots().is_empty());
        assert!(manager.events_after(0).len() <= MAX_SCOPE_EVENTS);
    }
}
