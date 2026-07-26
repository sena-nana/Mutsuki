use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mutsuki_runtime_contracts::{
    CrossDomainTaskRequest, DomainTaskHandle, RuntimeDomainId, TaskOutcome,
};
use mutsuki_runtime_core::{ReloadDecision, RuntimeResult, RuntimeStatistics, RuntimeStopState};
use mutsuki_runtime_sdk::HostServiceRegistry;

use crate::error::host_failure;
use crate::{
    HostRuntime, HostRuntimeCommand, HostRuntimeMetricsSnapshot, HostRuntimeReply,
    PreparedRuntimeReload, WorkerPoolSnapshot,
};

pub trait RuntimeDomainRouter: Send + Sync + std::fmt::Debug {
    fn route(&self, request: &CrossDomainTaskRequest) -> RuntimeResult<RuntimeDomainId>;
}

#[derive(Clone, Debug, Default)]
pub struct ExplicitRuntimeDomainRouter;

impl RuntimeDomainRouter for ExplicitRuntimeDomainRouter {
    fn route(&self, request: &CrossDomainTaskRequest) -> RuntimeResult<RuntimeDomainId> {
        Ok(request.target_domain.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeGroupLimits {
    pub max_domains: usize,
    pub max_worker_threads: usize,
    pub max_queued_batches: usize,
    pub max_inflight_bytes: usize,
}

impl Default for RuntimeGroupLimits {
    fn default() -> Self {
        Self {
            max_domains: 16,
            max_worker_threads: 256,
            max_queued_batches: 65_536,
            max_inflight_bytes: 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeDomainSnapshot {
    pub domain_id: RuntimeDomainId,
    pub stop_state: RuntimeStopState,
    pub statistics: RuntimeStatistics,
    pub execution_domains: Vec<WorkerPoolSnapshot>,
    pub metrics: HostRuntimeMetricsSnapshot,
}

#[derive(Clone)]
struct IdempotentDomainRequest {
    request_id: String,
    source_domain: RuntimeDomainId,
    target_domain: RuntimeDomainId,
    task_id: String,
    protocol_id: String,
    handle: DomainTaskHandle,
}

pub struct RuntimeGroupHost {
    domains: BTreeMap<RuntimeDomainId, HostRuntime>,
    shared_services: Arc<HostServiceRegistry>,
    router: Arc<dyn RuntimeDomainRouter>,
    limits: RuntimeGroupLimits,
    requests: Mutex<BTreeMap<String, IdempotentDomainRequest>>,
}

impl RuntimeGroupHost {
    pub fn new(
        shared_services: Arc<HostServiceRegistry>,
        router: Arc<dyn RuntimeDomainRouter>,
        limits: RuntimeGroupLimits,
    ) -> Self {
        Self {
            domains: BTreeMap::new(),
            shared_services,
            router,
            limits,
            requests: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn with_defaults(shared_services: Arc<HostServiceRegistry>) -> Self {
        Self::new(
            shared_services,
            Arc::new(ExplicitRuntimeDomainRouter),
            RuntimeGroupLimits::default(),
        )
    }

    pub fn shared_services(&self) -> &HostServiceRegistry {
        &self.shared_services
    }

    pub fn insert_domain(
        &mut self,
        domain_id: RuntimeDomainId,
        runtime: HostRuntime,
    ) -> RuntimeResult<()> {
        if self.domains.contains_key(&domain_id) {
            return Err(host_failure(
                "host.runtime_group.domain_duplicate",
                format!("runtime_domain.{domain_id}"),
            ));
        }
        if self.domains.len() >= self.limits.max_domains {
            return Err(host_failure(
                "host.runtime_group.domain_capacity",
                format!("runtime_domain.{domain_id}"),
            ));
        }
        let mut execution_domains = self.execution_domain_snapshots()?;
        execution_domains.extend(runtime.worker_pools()?);
        validate_group_capacity(&execution_domains, &self.limits)?;
        self.domains.insert(domain_id, runtime);
        Ok(())
    }

    pub fn domain(&self, domain_id: &RuntimeDomainId) -> Option<&HostRuntime> {
        self.domains.get(domain_id)
    }

    pub fn domain_mut(&mut self, domain_id: &RuntimeDomainId) -> Option<&mut HostRuntime> {
        self.domains.get_mut(domain_id)
    }

    pub fn submit_cross_domain(
        &self,
        request: CrossDomainTaskRequest,
    ) -> RuntimeResult<DomainTaskHandle> {
        request
            .validate()
            .map_err(mutsuki_runtime_core::RuntimeFailure::new)?;
        let target_domain = self.router.route(&request)?;
        if target_domain != request.target_domain {
            return Err(host_failure(
                "host.runtime_group.route_mismatch",
                format!(
                    "request.{}.target.{}.routed.{}",
                    request.request_id, request.target_domain, target_domain
                ),
            ));
        }
        {
            let requests = self
                .requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(existing) = requests.get(&request.idempotency_key) {
                if existing.request_id == request.request_id
                    && existing.source_domain == request.source_domain
                    && existing.target_domain == request.target_domain
                    && existing.task_id == request.task.task_id
                    && existing.protocol_id == request.task.protocol_id
                {
                    return Ok(existing.handle.clone());
                }
                return Err(host_failure(
                    "host.runtime_group.idempotency_conflict",
                    format!("idempotency_key.{}", request.idempotency_key),
                ));
            }
        }
        let runtime = self.domains.get(&target_domain).ok_or_else(|| {
            host_failure(
                "host.runtime_group.domain_missing",
                format!("runtime_domain.{target_domain}"),
            )
        })?;
        let task_id = request.task.task_id.clone();
        let protocol_id = request.task.protocol_id.clone();
        let reply = runtime.dispatch(HostRuntimeCommand::SubmitTask(Box::new(request.task)))?;
        let HostRuntimeReply::TaskSubmitted(task) = reply else {
            return Err(host_failure(
                "host.runtime_group.submit",
                format!("runtime_domain.{target_domain}"),
            ));
        };
        let handle = DomainTaskHandle {
            domain_id: target_domain.clone(),
            task,
        };
        let mut requests = self
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        requests.insert(
            request.idempotency_key,
            IdempotentDomainRequest {
                request_id: request.request_id,
                source_domain: request.source_domain,
                target_domain,
                task_id,
                protocol_id,
                handle: handle.clone(),
            },
        );
        Ok(handle)
    }

    pub fn wait_outcome(
        &self,
        handle: &DomainTaskHandle,
        timeout: Duration,
    ) -> RuntimeResult<Option<TaskOutcome>> {
        let runtime = self.domains.get(&handle.domain_id).ok_or_else(|| {
            host_failure(
                "host.runtime_group.domain_missing",
                format!("runtime_domain.{}", handle.domain_id),
            )
        })?;
        let states = runtime.wait_task_states(vec![handle.task.clone()], timeout)?;
        Ok(states.into_iter().next().and_then(|state| state.outcome))
    }

    pub fn cancel(&self, handle: &DomainTaskHandle) -> RuntimeResult<()> {
        let runtime = self.domains.get(&handle.domain_id).ok_or_else(|| {
            host_failure(
                "host.runtime_group.domain_missing",
                format!("runtime_domain.{}", handle.domain_id),
            )
        })?;
        match runtime.dispatch(HostRuntimeCommand::CancelTask(handle.task.clone()))? {
            HostRuntimeReply::TaskCancelled(_) => Ok(()),
            _ => Err(host_failure(
                "host.runtime_group.cancel",
                format!("runtime_domain.{}", handle.domain_id),
            )),
        }
    }

    pub fn reload_domain(
        &mut self,
        domain_id: &RuntimeDomainId,
        prepared: PreparedRuntimeReload,
        drain_timeout: Duration,
    ) -> RuntimeResult<ReloadDecision> {
        self.domains
            .get_mut(domain_id)
            .ok_or_else(|| {
                host_failure(
                    "host.runtime_group.domain_missing",
                    format!("runtime_domain.{domain_id}"),
                )
            })?
            .reload(prepared, drain_timeout)
    }

    pub fn begin_drain(&self) -> RuntimeResult<BTreeMap<RuntimeDomainId, RuntimeStopState>> {
        self.domains
            .iter()
            .map(|(domain_id, runtime)| Ok((domain_id.clone(), runtime.begin_drain()?)))
            .collect()
    }

    pub fn abort_domain(
        &self,
        domain_id: &RuntimeDomainId,
        reason: impl Into<String>,
    ) -> RuntimeResult<usize> {
        self.domains
            .get(domain_id)
            .ok_or_else(|| {
                host_failure(
                    "host.runtime_group.domain_missing",
                    format!("runtime_domain.{domain_id}"),
                )
            })?
            .abort(reason)
    }

    pub fn snapshots(&self) -> RuntimeResult<Vec<RuntimeDomainSnapshot>> {
        self.domains
            .iter()
            .map(|(domain_id, runtime)| {
                Ok(RuntimeDomainSnapshot {
                    domain_id: domain_id.clone(),
                    stop_state: runtime.stop_state()?,
                    statistics: runtime.statistics()?,
                    execution_domains: runtime.worker_pools()?,
                    metrics: runtime.metrics(),
                })
            })
            .collect()
    }

    fn execution_domain_snapshots(&self) -> RuntimeResult<Vec<WorkerPoolSnapshot>> {
        self.domains
            .values()
            .map(HostRuntime::worker_pools)
            .collect::<RuntimeResult<Vec<_>>>()
            .map(|groups| groups.into_iter().flatten().collect())
    }
}

fn validate_group_capacity(
    execution_domains: &[WorkerPoolSnapshot],
    limits: &RuntimeGroupLimits,
) -> RuntimeResult<()> {
    let worker_threads = execution_domains
        .iter()
        .map(|domain| domain.configured_threads)
        .sum::<usize>();
    let queued_batches = execution_domains
        .iter()
        .map(|domain| domain.queue_capacity)
        .sum::<usize>();
    let inflight_bytes = execution_domains
        .iter()
        .map(|domain| domain.max_inflight_bytes)
        .sum::<usize>();
    if worker_threads > limits.max_worker_threads
        || queued_batches > limits.max_queued_batches
        || inflight_bytes > limits.max_inflight_bytes
    {
        return Err(host_failure(
            "host.runtime_group.capacity",
            format!("threads.{worker_threads}.queues.{queued_batches}.bytes.{inflight_bytes}"),
        ));
    }
    Ok(())
}
