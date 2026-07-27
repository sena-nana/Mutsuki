use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use mutsuki_agent_contracts::{
    AGENT_CONTEXT_BUILD_PROTOCOL, AGENT_SESSION_APPEND_PROTOCOL, AgentContextBuildRequest,
    AgentSessionAppendRequest,
};
use mutsuki_bot_protocol::{QQBOT_GATEWAY_STATUS_PROTOCOL_ID, QqBotGatewayStatusRequest};
use mutsuki_runtime_contracts::{
    CrossDomainTaskRequest, DispatchLane, DomainTaskHandle, ExecutionClass, ObservabilityProfile,
    RunnerDescriptor, RunnerPurity, RunnerResult, RuntimeDomainId, RuntimeError, RuntimeProfile,
    RuntimeProfileMode, Task, TaskOutcome,
};
use mutsuki_runtime_host::{
    ExecutionDomainConfig, HostRuntime, HostRuntimeConfig, NativeRunner, RuntimeBootstrapper,
    RuntimeGroupHost, runner_manifest,
};
use mutsuki_runtime_sdk::{HostServiceRegistry, RuntimeFailure};
use serde_json::Value;

const BOT_DOMAIN_ID: &str = "bot-domain";
const AGENT_DOMAIN_ID: &str = "agent-domain";
const SHARED_DOMAIN_ID: &str = "bot-agent-shared-domain";
const REFERENCE_PLUGIN_ID: &str = "mutsuki.bot.runtime-domains.reference";
const ALL_WORKLOADS: &[BotReferenceWorkload] = &[
    BotReferenceWorkload::GatewayStatus,
    BotReferenceWorkload::AgentSessionAppend,
    BotReferenceWorkload::AgentContextBuild,
];
const BOT_WORKLOADS: &[BotReferenceWorkload] = &[BotReferenceWorkload::GatewayStatus];
const AGENT_WORKLOADS: &[BotReferenceWorkload] = &[
    BotReferenceWorkload::AgentSessionAppend,
    BotReferenceWorkload::AgentContextBuild,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotRuntimeTopology {
    SingleDomain,
    BotAgentDomains,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotReferenceWorkload {
    GatewayStatus,
    AgentSessionAppend,
    AgentContextBuild,
}

impl BotReferenceWorkload {
    pub fn protocol(self) -> &'static str {
        match self {
            Self::GatewayStatus => QQBOT_GATEWAY_STATUS_PROTOCOL_ID,
            Self::AgentSessionAppend => AGENT_SESSION_APPEND_PROTOCOL,
            Self::AgentContextBuild => AGENT_CONTEXT_BUILD_PROTOCOL,
        }
    }

    fn dispatch_lane(self) -> DispatchLane {
        match self {
            Self::GatewayStatus => DispatchLane::Interactive,
            Self::AgentSessionAppend | Self::AgentContextBuild => DispatchLane::Background,
        }
    }

    fn execution_class(self) -> ExecutionClass {
        match self {
            Self::GatewayStatus => ExecutionClass::Orchestration,
            Self::AgentSessionAppend | Self::AgentContextBuild => ExecutionClass::Script,
        }
    }
}

pub struct BotRuntimeDomainReference {
    topology: BotRuntimeTopology,
    group: RuntimeGroupHost,
}

impl BotRuntimeDomainReference {
    pub fn start(topology: BotRuntimeTopology) -> Result<Self, String> {
        let shared_services = Arc::new(HostServiceRegistry::new());
        shared_services.freeze();
        let mut group = RuntimeGroupHost::with_defaults(shared_services.clone());
        let domains: &[(&str, &[BotReferenceWorkload], usize)] = match topology {
            BotRuntimeTopology::SingleDomain => &[(SHARED_DOMAIN_ID, ALL_WORKLOADS, 2)],
            BotRuntimeTopology::BotAgentDomains => &[
                (BOT_DOMAIN_ID, BOT_WORKLOADS, 1),
                (AGENT_DOMAIN_ID, AGENT_WORKLOADS, 1),
            ],
        };
        for &(id, workloads, threads) in domains {
            group
                .insert_domain(
                    domain_id(id)?,
                    build_runtime(shared_services.clone(), id, workloads, threads)?,
                )
                .map_err(|error| error.to_string())?;
        }

        Ok(Self { topology, group })
    }

    pub fn submit(
        &self,
        request_id: impl Into<String>,
        workload: BotReferenceWorkload,
        payload: Value,
    ) -> Result<DomainTaskHandle, String> {
        let request_id = request_id.into();
        let source_domain = self.route(BotReferenceWorkload::GatewayStatus)?;
        let target_domain = self.route(workload)?;
        let mut task = Task::new(request_id.clone(), workload.protocol(), payload);
        task.dispatch_lane = workload.dispatch_lane();
        self.group
            .submit_cross_domain(CrossDomainTaskRequest {
                request_id: request_id.clone(),
                source_domain,
                target_domain,
                task,
                timeout_ms: 5_000,
                idempotency_key: format!("{request_id}:{}", workload.protocol()),
                max_attempts: 1,
            })
            .map_err(|error| error.to_string())
    }

    pub fn wait_outcome(
        &self,
        handle: &DomainTaskHandle,
        timeout: Duration,
    ) -> Result<Option<TaskOutcome>, String> {
        self.group
            .wait_outcome(handle, timeout)
            .map_err(|error| error.to_string())
    }

    fn route(&self, workload: BotReferenceWorkload) -> Result<RuntimeDomainId, String> {
        let id = match self.topology {
            BotRuntimeTopology::SingleDomain => SHARED_DOMAIN_ID,
            BotRuntimeTopology::BotAgentDomains => match workload {
                BotReferenceWorkload::GatewayStatus => BOT_DOMAIN_ID,
                BotReferenceWorkload::AgentSessionAppend
                | BotReferenceWorkload::AgentContextBuild => AGENT_DOMAIN_ID,
            },
        };
        domain_id(id)
    }

    pub fn is_single_domain(&self) -> bool {
        self.topology == BotRuntimeTopology::SingleDomain
    }

    pub fn group(&self) -> &RuntimeGroupHost {
        &self.group
    }
}

fn build_runtime(
    shared_services: Arc<HostServiceRegistry>,
    profile_id: &str,
    workloads: &[BotReferenceWorkload],
    threads: usize,
) -> Result<HostRuntime, String> {
    let descriptors = workloads
        .iter()
        .copied()
        .map(descriptor)
        .collect::<Vec<_>>();
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(runner_manifest(REFERENCE_PLUGIN_ID, descriptors.clone()));
    bootstrapper
        .use_shared_services(shared_services)
        .map_err(|error| error.to_string())?;

    for (workload, descriptor) in workloads.iter().copied().zip(descriptors) {
        bootstrapper.register_runner(Box::new(NativeRunner::new(
            descriptor,
            move |_context, task| {
                let task_id = task.task_id.clone();
                let payload: Value = task.payload.into();
                let output = execute_reference_workload(workload, &payload)
                    .map_err(|message| reference_failure(workload, message))?;
                let mut result = RunnerResult::completed(task_id);
                result.output = Some(output);
                Ok(result)
            },
        )));
    }

    bootstrapper
        .into_host_runtime_with_config(
            profile(profile_id),
            HostRuntimeConfig {
                event_driven: true,
                execution_domains: vec![ExecutionDomainConfig::new(
                    format!("{profile_id}-workers"),
                    all_execution_classes(),
                    threads,
                )],
                ..HostRuntimeConfig::default()
            },
        )
        .map_err(|error| error.to_string())
}

fn execute_reference_workload(
    workload: BotReferenceWorkload,
    payload: &Value,
) -> Result<Value, String> {
    match workload {
        BotReferenceWorkload::GatewayStatus => {
            serde_json::from_value::<QqBotGatewayStatusRequest>(payload.clone())
                .map_err(|error| format!("invalid QQ gateway status request: {error}"))?;
            Ok(serde_json::json!({
                "connected": true,
                "protocol_id": QQBOT_GATEWAY_STATUS_PROTOCOL_ID,
            }))
        }
        BotReferenceWorkload::AgentSessionAppend => {
            execute_agent_contract::<AgentSessionAppendRequest>(payload)
        }
        BotReferenceWorkload::AgentContextBuild => {
            execute_agent_contract::<AgentContextBuildRequest>(payload)
        }
    }
}

fn execute_agent_contract<T>(payload: &Value) -> Result<Value, String>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let iterations = payload
        .get("iterations")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| "agent reference payload requires positive iterations".to_string())?;
    let request = payload
        .get("request")
        .cloned()
        .ok_or_else(|| "agent reference payload requires request".to_string())?;
    let mut output = Value::Null;
    for _ in 0..iterations {
        let encoded = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        let parsed: T = serde_json::from_slice(&encoded).map_err(|error| error.to_string())?;
        output = serde_json::to_value(parsed).map_err(|error| error.to_string())?;
    }
    Ok(serde_json::json!({
        "iterations": iterations,
        "request": output,
    }))
}

fn reference_failure(workload: BotReferenceWorkload, message: String) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(
        "mutsuki.bot.reference.invalid_input",
        "mutsuki.bot.runtime-domain-reference",
        format!("{}.{}", workload.protocol(), message),
    ))
}

fn descriptor(workload: BotReferenceWorkload) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: format!("{}.runner", workload.protocol()),
        plugin_id: REFERENCE_PLUGIN_ID.into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec![workload.protocol().into()],
        purity: RunnerPurity::Pure,
        execution_class: workload.execution_class(),
        invocation_mode: Default::default(),
        concurrency: Default::default(),
        input_schema: serde_json::json!({}),
        output_schema: serde_json::json!({}),
        batch: Default::default(),
        payload: Default::default(),
        resources: Default::default(),
        ordering: Default::default(),
        control: Default::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("runner:{}", workload.protocol())],
    }
}

fn profile(profile_id: &str) -> RuntimeProfile {
    RuntimeProfile {
        profile_id: format!("bot-reference-{profile_id}"),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![REFERENCE_PLUGIN_ID.into()],
        bindings: BTreeMap::new(),
        plugin_deployments: BTreeMap::new(),
        observability: ObservabilityProfile::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: false,
    }
}

fn all_execution_classes() -> Vec<ExecutionClass> {
    vec![
        ExecutionClass::Orchestration,
        ExecutionClass::Io,
        ExecutionClass::Cpu,
        ExecutionClass::Blocking,
        ExecutionClass::Script,
    ]
}

fn domain_id(value: &str) -> Result<RuntimeDomainId, String> {
    RuntimeDomainId::new(value).map_err(|error| format!("{error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::AgentMessage;
    use serde_json::json;

    fn payload(workload: BotReferenceWorkload) -> Value {
        match workload {
            BotReferenceWorkload::GatewayStatus => json!({}),
            BotReferenceWorkload::AgentSessionAppend => json!({
                "iterations": 1,
                "request": {
                    "session_id": "issue43-session",
                    "messages": [AgentMessage::user("hello")]
                }
            }),
            BotReferenceWorkload::AgentContextBuild => json!({
                "iterations": 1,
                "request": {
                    "profile_id": "issue43-agent",
                    "messages": [AgentMessage::user("build context")],
                    "session_id": "issue43-session",
                    "max_context_tokens": 4096
                }
            }),
        }
    }

    #[test]
    fn bot_agent_reference_runs_owner_contracts_and_isolates_agent_abort() {
        let reference =
            BotRuntimeDomainReference::start(BotRuntimeTopology::BotAgentDomains).unwrap();
        assert_eq!(
            reference
                .group()
                .snapshots()
                .unwrap()
                .into_iter()
                .map(|snapshot| snapshot.domain_id.to_string())
                .collect::<Vec<_>>(),
            vec![AGENT_DOMAIN_ID, BOT_DOMAIN_ID]
        );

        for (index, workload) in [
            BotReferenceWorkload::GatewayStatus,
            BotReferenceWorkload::AgentSessionAppend,
            BotReferenceWorkload::AgentContextBuild,
        ]
        .into_iter()
        .enumerate()
        {
            let handle = reference
                .submit(
                    format!("owner-contract-{index}"),
                    workload,
                    payload(workload),
                )
                .unwrap();
            assert!(matches!(
                reference
                    .wait_outcome(&handle, Duration::from_secs(2))
                    .unwrap(),
                Some(TaskOutcome::Completed {
                    output: Some(_),
                    ..
                })
            ));
        }

        reference
            .group()
            .abort_domain(&domain_id(AGENT_DOMAIN_ID).unwrap(), "test.agent.abort")
            .unwrap();
        let handle = reference
            .submit(
                "gateway-after-agent-abort",
                BotReferenceWorkload::GatewayStatus,
                payload(BotReferenceWorkload::GatewayStatus),
            )
            .unwrap();
        assert!(matches!(
            reference
                .wait_outcome(&handle, Duration::from_secs(2))
                .unwrap(),
            Some(TaskOutcome::Completed { .. })
        ));
    }

    #[test]
    fn invalid_owner_contract_payloads_fail_as_tasks() {
        let reference =
            BotRuntimeDomainReference::start(BotRuntimeTopology::BotAgentDomains).unwrap();
        for (index, workload) in [
            BotReferenceWorkload::AgentSessionAppend,
            BotReferenceWorkload::AgentContextBuild,
        ]
        .into_iter()
        .enumerate()
        {
            let handle = reference
                .submit(format!("invalid-{index}"), workload, json!({}))
                .unwrap();
            assert!(matches!(
                reference
                    .wait_outcome(&handle, Duration::from_secs(2))
                    .unwrap(),
                Some(TaskOutcome::Failed { .. })
            ));
        }
    }

    #[test]
    fn topologies_keep_the_same_total_worker_budget() {
        for topology in [
            BotRuntimeTopology::SingleDomain,
            BotRuntimeTopology::BotAgentDomains,
        ] {
            let reference = BotRuntimeDomainReference::start(topology).unwrap();
            let workers = reference
                .group()
                .snapshots()
                .unwrap()
                .iter()
                .flat_map(|snapshot| &snapshot.execution_domains)
                .map(|domain| domain.configured_threads)
                .sum::<usize>();
            assert_eq!(workers, 2);
        }
    }
}
