use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use mutsuki_agent_contracts::{
    AGENT_RUN_PROTOCOL, AgentAffinity, AgentError, AgentMigrationPolicy, AgentSideEffectClass,
    AgentTaskPlacement,
};

use crate::{AgentRemoteSubagentRequest, AgentRemoteTaskRequest};
use mutsuki_distributed_contracts::{
    DirectDataRef, GlobalTaskId, LocalTaskOutcome, RemoteResult, TaskPlacement,
};
use mutsuki_distributed_runtime::Coordinator;
use mutsuki_runtime_contracts::{
    ContentId, ExecutionMobility, PortabilityCapability, PortableTask, RecoveryMode, RefId,
    RequirementSet, ResourcePersistence, RetrySafety, SchemaIdentity, Task,
    TaskAcceptanceDurability,
};
use sha2::{Digest, Sha256};

pub type AgentDistributedFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, AgentError>> + Send + 'a>>;

pub trait AgentDistributedCoordinator: Send + Sync {
    fn submit<'a>(
        &'a self,
        global_task_id: GlobalTaskId,
        portable: PortableTask,
        requirements: RequirementSet,
        direct_inputs: Vec<DirectDataRef>,
    ) -> AgentDistributedFuture<'a, TaskPlacement>;

    fn cancel<'a>(&'a self, global_task_id: &'a GlobalTaskId) -> AgentDistributedFuture<'a, ()>;

    fn outcome<'a>(
        &'a self,
        global_task_id: &'a GlobalTaskId,
    ) -> AgentDistributedFuture<'a, Option<LocalTaskOutcome>>;

    fn accept_result(&self, result: RemoteResult) -> Result<RemoteResult, AgentError>;
}

impl AgentDistributedCoordinator for Coordinator {
    fn submit<'a>(
        &'a self,
        global_task_id: GlobalTaskId,
        portable: PortableTask,
        requirements: RequirementSet,
        direct_inputs: Vec<DirectDataRef>,
    ) -> AgentDistributedFuture<'a, TaskPlacement> {
        Box::pin(async move {
            Coordinator::submit(self, global_task_id, portable, requirements, direct_inputs)
                .await
                .map_err(distributed_error)
        })
    }

    fn cancel<'a>(&'a self, global_task_id: &'a GlobalTaskId) -> AgentDistributedFuture<'a, ()> {
        Box::pin(async move {
            Coordinator::cancel(self, global_task_id)
                .await
                .map_err(distributed_error)
        })
    }

    fn outcome<'a>(
        &'a self,
        global_task_id: &'a GlobalTaskId,
    ) -> AgentDistributedFuture<'a, Option<LocalTaskOutcome>> {
        Box::pin(async move {
            Coordinator::outcome(self, global_task_id)
                .await
                .map_err(distributed_error)
        })
    }

    fn accept_result(&self, result: RemoteResult) -> Result<RemoteResult, AgentError> {
        Coordinator::accept_result(self, result).map_err(distributed_error)
    }
}

pub struct AgentDistributedTaskClient<C> {
    coordinator: Arc<C>,
    subagents: Mutex<BTreeMap<String, BTreeSet<GlobalTaskId>>>,
}

impl<C> AgentDistributedTaskClient<C> {
    pub fn new(coordinator: Arc<C>) -> Self {
        Self {
            coordinator,
            subagents: Mutex::new(BTreeMap::new()),
        }
    }
}

impl<C: AgentDistributedCoordinator> AgentDistributedTaskClient<C> {
    pub async fn submit_task(
        &self,
        request: AgentRemoteTaskRequest,
    ) -> Result<TaskPlacement, AgentError> {
        validate_task_request(&request)?;
        let global_task_id = GlobalTaskId(request.global_task_id.clone());
        let (portable, requirements) = portable_task(
            request.task_id,
            request.protocol_id,
            request.task_schema_id,
            request.task_schema_version,
            request.payload,
            &request.placement,
        )?;
        self.coordinator
            .submit(global_task_id, portable, requirements, Vec::new())
            .await
    }

    pub async fn submit_subagent(
        &self,
        request: AgentRemoteSubagentRequest,
    ) -> Result<TaskPlacement, AgentError> {
        if request.parent_session_id.trim().is_empty()
            || request.child_session_id.trim().is_empty()
            || request.parent_session_id == request.child_session_id
        {
            return Err(AgentError::invalid_input(
                "subagent requires distinct non-empty parent and child session ids",
            ));
        }
        let global_task_id = GlobalTaskId(format!(
            "agent-subagent:{}:{}",
            request.parent_session_id, request.child_session_id
        ));
        let payload = serde_json::to_value(&request.run)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let (portable, requirements) = portable_task(
            format!("subagent:{}", request.child_session_id),
            AGENT_RUN_PROTOCOL.to_owned(),
            AGENT_RUN_PROTOCOL.to_owned(),
            "1.0.0".to_owned(),
            payload,
            &request.placement,
        )?;
        let placement = self
            .coordinator
            .submit(global_task_id.clone(), portable, requirements, Vec::new())
            .await?;
        self.subagents
            .lock()
            .expect("agent distributed subagent mutex")
            .entry(request.parent_session_id)
            .or_default()
            .insert(global_task_id);
        Ok(placement)
    }

    pub async fn cancel_task(&self, global_task_id: &str) -> Result<(), AgentError> {
        validate_identifier(global_task_id, "global task id")?;
        self.coordinator
            .cancel(&GlobalTaskId(global_task_id.to_owned()))
            .await
    }

    pub async fn cancel_subagents(&self, parent_session_id: &str) -> Result<usize, AgentError> {
        validate_identifier(parent_session_id, "parent session id")?;
        let task_ids = self
            .subagents
            .lock()
            .expect("agent distributed subagent mutex")
            .remove(parent_session_id)
            .unwrap_or_default();
        for task_id in &task_ids {
            self.coordinator.cancel(task_id).await?;
        }
        Ok(task_ids.len())
    }

    pub async fn recover_result(
        &self,
        global_task_id: &str,
    ) -> Result<Option<LocalTaskOutcome>, AgentError> {
        validate_identifier(global_task_id, "global task id")?;
        self.coordinator
            .outcome(&GlobalTaskId(global_task_id.to_owned()))
            .await
    }

    pub fn accept_remote_result(&self, result: RemoteResult) -> Result<RemoteResult, AgentError> {
        self.coordinator.accept_result(result)
    }
}

fn validate_task_request(request: &AgentRemoteTaskRequest) -> Result<(), AgentError> {
    validate_identifier(&request.global_task_id, "global task id")?;
    validate_identifier(&request.task_id, "task id")?;
    validate_identifier(&request.protocol_id, "protocol id")?;
    validate_identifier(&request.task_schema_id, "task schema id")?;
    validate_identifier(&request.task_schema_version, "task schema version")
}

fn validate_identifier(value: &str, label: &str) -> Result<(), AgentError> {
    if value.trim().is_empty() {
        Err(AgentError::invalid_input(format!("{label} is required")))
    } else {
        Ok(())
    }
}

fn portable_task(
    task_id: String,
    protocol_id: String,
    task_schema_id: String,
    task_schema_version: String,
    payload: serde_json::Value,
    placement: &AgentTaskPlacement,
) -> Result<(PortableTask, RequirementSet), AgentError> {
    if matches!(placement.affinity, AgentAffinity::Required(ref node) if node.trim().is_empty()) {
        return Err(AgentError::invalid_input(
            "required affinity node id cannot be empty",
        ));
    }
    let payload_bytes = serde_json::to_vec(&payload)
        .map_err(|error| AgentError::invalid_input(error.to_string()))?;
    let digest = hex::encode(Sha256::digest(&payload_bytes));
    let mut task = Task::new(task_id, protocol_id, payload);
    task.input_refs = placement
        .required_resource_refs
        .iter()
        .cloned()
        .map(RefId::from)
        .collect();
    let capability = portability_capability(placement);
    let portable = PortableTask::new(
        task,
        SchemaIdentity::new(task_schema_id, task_schema_version),
        ContentId::new("sha256", digest, payload_bytes.len() as u64, "json"),
        capability,
    );
    let mut requirements = RequirementSet {
        custom: placement.required_capabilities.iter().cloned().collect(),
        ..RequirementSet::default()
    };
    if let AgentAffinity::Required(node_id) = &placement.affinity {
        requirements
            .custom
            .insert(format!("mutsuki.node:{node_id}"));
    }
    Ok((portable, requirements))
}

fn portability_capability(placement: &AgentTaskPlacement) -> PortabilityCapability {
    let remote_safe = placement.permits_remote_execution() && placement.data_locality.is_empty();
    let (mobility, retry_safety, recovery) = if !remote_safe {
        (
            ExecutionMobility::LocalOnly,
            RetrySafety::Unsafe,
            RecoveryMode::Unavailable,
        )
    } else {
        match placement.migration {
            AgentMigrationPolicy::Never => (
                ExecutionMobility::Portable,
                side_effect_retry_safety(&placement.side_effect),
                RecoveryMode::Unavailable,
            ),
            AgentMigrationPolicy::CheckpointOnly => (
                ExecutionMobility::Checkpointable,
                RetrySafety::Verifiable,
                RecoveryMode::RestoreCheckpoint,
            ),
            AgentMigrationPolicy::ReconcileIdempotent => (
                ExecutionMobility::Restartable,
                side_effect_retry_safety(&placement.side_effect),
                RecoveryMode::RestartFromInput,
            ),
        }
    };
    PortabilityCapability {
        mobility,
        retry_safety,
        task_acceptance: if remote_safe {
            TaskAcceptanceDurability::Persisted
        } else {
            TaskAcceptanceDurability::Volatile
        },
        resource_persistence: ResourcePersistence::Ephemeral,
        recovery,
    }
}

fn side_effect_retry_safety(side_effect: &AgentSideEffectClass) -> RetrySafety {
    match side_effect {
        AgentSideEffectClass::Pure | AgentSideEffectClass::Idempotent => RetrySafety::Idempotent,
        AgentSideEffectClass::ApprovalBound | AgentSideEffectClass::NonIdempotent => {
            RetrySafety::Unsafe
        }
    }
}

fn distributed_error(error: mutsuki_distributed_contracts::DistributedError) -> AgentError {
    AgentError::new(
        format!("agent.distributed.{:?}", error.kind).to_lowercase(),
        error
            .detail
            .unwrap_or_else(|| error.public_message.to_owned()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::{AgentMessage, AgentRunRequest};
    use mutsuki_distributed_contracts::{
        DISTRIBUTED_PROTOCOL_MAJOR, LocalTaskSnapshot, NodeId, PlacementKind, RemoteResult,
        WorkerAdvertisement, WorkerHealth,
    };
    use mutsuki_distributed_host_adapter::{HostAdapter, HostFuture};
    use mutsuki_distributed_runtime::{
        RemoteWorker, ResourceLocalizer, WireRemoteWorker, WorkerEndpoint, WorkerFuture,
        WorkerRegistry, WorkerRequestDispatcher, WorkerTransport,
    };
    use mutsuki_runtime_contracts::{
        CancelPolicy, CapabilitySet, PortabilityCatalog, RuntimeEvent, TaskBatch, TaskHandle,
        TaskId, TaskPortabilityDescriptor,
    };

    #[derive(Default)]
    struct FakeCoordinator {
        submitted: Mutex<Vec<(GlobalTaskId, PortableTask, RequirementSet)>>,
        cancelled: Mutex<Vec<GlobalTaskId>>,
    }

    impl AgentDistributedCoordinator for FakeCoordinator {
        fn submit<'a>(
            &'a self,
            global_task_id: GlobalTaskId,
            portable: PortableTask,
            requirements: RequirementSet,
            _direct_inputs: Vec<DirectDataRef>,
        ) -> AgentDistributedFuture<'a, TaskPlacement> {
            self.submitted
                .lock()
                .unwrap()
                .push((global_task_id.clone(), portable, requirements));
            Box::pin(async move {
                Ok(TaskPlacement {
                    kind: PlacementKind::Remote,
                    global_task_id,
                    attempt: 1,
                    node_id: NodeId("worker".into()),
                    local_handle: TaskHandle {
                        task_id: "worker-task".into(),
                        protocol_id: AGENT_RUN_PROTOCOL.into(),
                        target_binding_id: None,
                        cancel_policy: CancelPolicy::Cascade,
                        trace_id: None,
                        correlation_id: None,
                    },
                })
            })
        }

        fn cancel<'a>(
            &'a self,
            global_task_id: &'a GlobalTaskId,
        ) -> AgentDistributedFuture<'a, ()> {
            self.cancelled.lock().unwrap().push(global_task_id.clone());
            Box::pin(async { Ok(()) })
        }

        fn outcome<'a>(
            &'a self,
            global_task_id: &'a GlobalTaskId,
        ) -> AgentDistributedFuture<'a, Option<LocalTaskOutcome>> {
            let task_id = global_task_id.0.clone();
            Box::pin(async move {
                Ok(Some(LocalTaskOutcome {
                    task_id: task_id.into(),
                    status: "completed".into(),
                    output_ref: Some("resource:result".into()),
                    reason: None,
                    error_code: None,
                }))
            })
        }

        fn accept_result(&self, result: RemoteResult) -> Result<RemoteResult, AgentError> {
            if result.worker_node.0 == "worker" && result.attempt == 1 {
                Ok(result)
            } else {
                Err(AgentError::new(
                    "agent.distributed.attemptstale",
                    "remote result belongs to a stale attempt",
                ))
            }
        }
    }

    fn placement() -> AgentTaskPlacement {
        AgentTaskPlacement {
            required_capabilities: vec!["coding".into()],
            affinity: AgentAffinity::Required("worker".into()),
            data_locality: Vec::new(),
            latency_class: "interactive".into(),
            cost_class: "standard".into(),
            remote_execution_allowed: true,
            migration: AgentMigrationPolicy::ReconcileIdempotent,
            side_effect: AgentSideEffectClass::Pure,
            required_resource_refs: Vec::new(),
        }
    }

    #[test]
    fn subagent_submission_uses_portable_runtime_contract_and_parent_cancel_cascades() {
        let coordinator = Arc::new(FakeCoordinator::default());
        let client = AgentDistributedTaskClient::new(coordinator.clone());
        let mut run =
            AgentRunRequest::new("coding", vec![AgentMessage::user("implement the change")]);
        run.session_id = Some("child".into());
        let placed =
            futures::executor::block_on(client.submit_subagent(AgentRemoteSubagentRequest {
                parent_session_id: "parent".into(),
                child_session_id: "child".into(),
                run,
                placement: placement(),
            }))
            .unwrap();
        assert_eq!(placed.kind, PlacementKind::Remote);

        let submitted = coordinator.submitted.lock().unwrap();
        let (_, portable, requirements) = &submitted[0];
        assert_eq!(portable.task.protocol_id, AGENT_RUN_PROTOCOL);
        assert_eq!(portable.capability.mobility, ExecutionMobility::Restartable);
        assert_eq!(
            portable.capability.resource_persistence,
            ResourcePersistence::Ephemeral
        );
        assert!(requirements.custom.contains("coding"));
        assert!(requirements.custom.contains("mutsuki.node:worker"));
        drop(submitted);

        let outcome =
            futures::executor::block_on(client.recover_result("agent-subagent:parent:child"))
                .unwrap()
                .unwrap();
        assert_eq!(
            outcome.output_ref.as_ref().map(|id| id.as_str()),
            Some("resource:result")
        );
        assert_eq!(
            futures::executor::block_on(client.cancel_subagents("parent")).unwrap(),
            1
        );
        assert_eq!(
            coordinator.cancelled.lock().unwrap().as_slice(),
            &[GlobalTaskId("agent-subagent:parent:child".into())]
        );
    }

    #[test]
    fn unmaterialized_resource_refs_mark_portable_task_local_only() {
        let mut placement = placement();
        placement.required_resource_refs = vec!["resource:workspace".into()];
        let coordinator = Arc::new(FakeCoordinator::default());
        let client = AgentDistributedTaskClient::new(coordinator.clone());
        futures::executor::block_on(client.submit_task(AgentRemoteTaskRequest {
            global_task_id: "resource-task".into(),
            task_id: "resource-task".into(),
            protocol_id: "example/read@1".into(),
            task_schema_id: "example.read".into(),
            task_schema_version: "1.0.0".into(),
            payload: serde_json::json!({"path": "src/lib.rs"}),
            placement,
        }))
        .unwrap();

        let submitted = coordinator.submitted.lock().unwrap();
        let portable = &submitted[0].1;
        assert_eq!(portable.capability.mobility, ExecutionMobility::LocalOnly);
        assert_eq!(
            portable.task.input_refs,
            vec![RefId::from("resource:workspace")]
        );
        assert_eq!(
            portable.capability.resource_persistence,
            ResourcePersistence::Ephemeral
        );
    }

    #[test]
    fn unresolved_data_locality_marks_portable_task_local_only() {
        let mut placement = placement();
        placement.data_locality = vec!["sha256:workspace-snapshot".into()];
        let capability = portability_capability(&placement);
        assert_eq!(capability.mobility, ExecutionMobility::LocalOnly);
        assert_eq!(capability.retry_safety, RetrySafety::Unsafe);
    }

    #[test]
    fn unsafe_effect_is_local_only_and_stale_remote_result_is_rejected() {
        let mut placement = placement();
        placement.side_effect = AgentSideEffectClass::NonIdempotent;
        let coordinator = Arc::new(FakeCoordinator::default());
        let client = AgentDistributedTaskClient::new(coordinator.clone());
        futures::executor::block_on(client.submit_task(AgentRemoteTaskRequest {
            global_task_id: "task".into(),
            task_id: "task".into(),
            protocol_id: "example/write@1".into(),
            task_schema_id: "example.write".into(),
            task_schema_version: "1.0.0".into(),
            payload: serde_json::json!({"value": 1}),
            placement,
        }))
        .unwrap();
        assert_eq!(
            coordinator.submitted.lock().unwrap()[0]
                .1
                .capability
                .mobility,
            ExecutionMobility::LocalOnly
        );

        let stale = client
            .accept_remote_result(RemoteResult {
                global_task_id: GlobalTaskId("task".into()),
                attempt: 2,
                worker_node: NodeId("old-worker".into()),
                outcome: None,
                direct_outputs: Vec::new(),
            })
            .unwrap_err();
        assert_eq!(stale.code, "agent.distributed.attemptstale");
    }

    #[derive(Default)]
    struct RecordingHost {
        submitted: Mutex<Vec<Task>>,
        cancelled: Mutex<Vec<TaskId>>,
    }

    impl HostAdapter for RecordingHost {
        fn submit_batch(&self, batch: TaskBatch) -> HostFuture<'_, Vec<TaskHandle>> {
            Box::pin(async move {
                let handles = batch
                    .tasks
                    .iter()
                    .map(|task| TaskHandle {
                        task_id: task.task_id.clone(),
                        protocol_id: task.protocol_id.clone(),
                        target_binding_id: task.target_binding_id.clone(),
                        cancel_policy: CancelPolicy::Cascade,
                        trace_id: task.trace_id.clone(),
                        correlation_id: task.correlation_id.clone(),
                    })
                    .collect();
                self.submitted.lock().unwrap().extend(batch.tasks);
                Ok(handles)
            })
        }

        fn cancel(&self, handle: &TaskHandle) -> HostFuture<'_, ()> {
            let task_id = handle.task_id.clone();
            Box::pin(async move {
                self.cancelled.lock().unwrap().push(task_id);
                Ok(())
            })
        }

        fn snapshots(&self) -> HostFuture<'_, Vec<LocalTaskSnapshot>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn outcome(&self, handle: &TaskHandle) -> HostFuture<'_, Option<LocalTaskOutcome>> {
            let task_id = handle.task_id.clone();
            Box::pin(async move {
                Ok(Some(LocalTaskOutcome {
                    task_id,
                    status: "completed".into(),
                    output_ref: Some("resource:remote-result".into()),
                    reason: None,
                    error_code: None,
                }))
            })
        }

        fn events_after(&self, _sequence: u64, _limit: usize) -> HostFuture<'_, Vec<RuntimeEvent>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn begin_drain(&self) -> HostFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn health(&self) -> HostFuture<'_, String> {
            Box::pin(async { Ok("ok".into()) })
        }
    }

    struct NoopLocalizer;

    impl ResourceLocalizer for NoopLocalizer {
        fn localize<'a>(&'a self, resources: &'a [DirectDataRef]) -> WorkerFuture<'a, ()> {
            Box::pin(async move {
                assert!(resources.is_empty());
                Ok(())
            })
        }
    }

    struct LoopbackTransport {
        dispatcher: Arc<WorkerRequestDispatcher>,
    }

    impl WorkerTransport for LoopbackTransport {
        fn round_trip(&self, request: Vec<u8>) -> WorkerFuture<'_, Vec<u8>> {
            Box::pin(async move { self.dispatcher.dispatch(&request).await })
        }
    }

    fn real_coordinator() -> (Arc<Coordinator>, Arc<RecordingHost>, Arc<RecordingHost>) {
        let origin_host = Arc::new(RecordingHost::default());
        let worker_host = Arc::new(RecordingHost::default());
        let worker_node = NodeId("worker".into());
        let endpoint = Arc::new(WorkerEndpoint::new(
            worker_node.clone(),
            worker_host.clone(),
            Arc::new(NoopLocalizer),
        ));
        let dispatcher = Arc::new(WorkerRequestDispatcher::new(
            endpoint as Arc<dyn RemoteWorker>,
        ));
        let wire: Arc<dyn RemoteWorker> =
            Arc::new(WireRemoteWorker::new(Arc::new(LoopbackTransport {
                dispatcher,
            })));

        let expected = placement();
        let mut capabilities = CapabilitySet::default();
        capabilities.custom.insert("coding".into());
        capabilities.custom.insert("mutsuki.node:worker".into());
        let mut registry = WorkerRegistry::new(1).unwrap();
        registry
            .register(WorkerAdvertisement {
                node_id: worker_node.clone(),
                protocol_major: DISTRIBUTED_PROTOCOL_MAJOR,
                snapshot_version: 1,
                capabilities,
                portability: PortabilityCatalog {
                    tasks: vec![TaskPortabilityDescriptor {
                        protocol_id: AGENT_RUN_PROTOCOL.into(),
                        task_schema: SchemaIdentity::new(AGENT_RUN_PROTOCOL, "1.0.0"),
                        checkpoint_schema: None,
                        capability: portability_capability(&expected),
                    }],
                    resources: Vec::new(),
                },
                runners: Vec::new(),
                localized_content: BTreeSet::new(),
                health: WorkerHealth::Ready,
            })
            .unwrap();
        let coordinator = Coordinator::new(
            NodeId("origin".into()),
            origin_host.clone(),
            Arc::new(Mutex::new(registry)),
            [(worker_node, wire)].into_iter().collect(),
            8,
            0,
        )
        .unwrap();
        (Arc::new(coordinator), origin_host, worker_host)
    }

    #[test]
    fn agent_subagent_executes_through_real_distributed_worker() {
        let (coordinator, origin_host, worker_host) = real_coordinator();
        let client = AgentDistributedTaskClient::new(coordinator);
        let mut run =
            AgentRunRequest::new("coding", vec![AgentMessage::user("inspect the workspace")]);
        run.session_id = Some("remote-child".into());
        let placed =
            futures::executor::block_on(client.submit_subagent(AgentRemoteSubagentRequest {
                parent_session_id: "remote-parent".into(),
                child_session_id: "remote-child".into(),
                run,
                placement: placement(),
            }))
            .unwrap();

        assert_eq!(placed.kind, PlacementKind::Remote);
        assert_eq!(placed.node_id, NodeId("worker".into()));
        assert!(origin_host.submitted.lock().unwrap().is_empty());
        let submitted = worker_host.submitted.lock().unwrap();
        assert_eq!(submitted.len(), 1);
        assert_eq!(submitted[0].protocol_id, AGENT_RUN_PROTOCOL);
        drop(submitted);

        let outcome = futures::executor::block_on(
            client.recover_result("agent-subagent:remote-parent:remote-child"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            outcome.output_ref.as_ref().map(|id| id.as_str()),
            Some("resource:remote-result")
        );
        assert_eq!(
            futures::executor::block_on(client.cancel_subagents("remote-parent")).unwrap(),
            1
        );
        assert_eq!(
            worker_host.cancelled.lock().unwrap().as_slice(),
            &[TaskId::from(
                "agent-subagent:remote-parent:remote-child:attempt:1",
            )]
        );
    }

    #[test]
    fn resource_bound_subagent_falls_back_to_origin_host() {
        let (coordinator, origin_host, worker_host) = real_coordinator();
        let client = AgentDistributedTaskClient::new(coordinator);
        let mut resource_placement = placement();
        resource_placement.required_resource_refs = vec!["resource:workspace".into()];
        let placed =
            futures::executor::block_on(client.submit_subagent(AgentRemoteSubagentRequest {
                parent_session_id: "local-parent".into(),
                child_session_id: "local-child".into(),
                run: AgentRunRequest::new("coding", vec![AgentMessage::user("read the workspace")]),
                placement: resource_placement,
            }))
            .unwrap();

        assert_eq!(placed.kind, PlacementKind::Local);
        assert_eq!(placed.node_id, NodeId("origin".into()));
        assert!(worker_host.submitted.lock().unwrap().is_empty());
        let submitted = origin_host.submitted.lock().unwrap();
        assert_eq!(submitted.len(), 1);
        assert_eq!(
            submitted[0].input_refs,
            vec![RefId::from("resource:workspace")]
        );
    }
}
