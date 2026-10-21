use std::sync::Arc;

use mutsuki_agent_adapter_api::{CredentialBroker, CredentialFuture, CredentialValue};
use mutsuki_agent_contracts::{
    CREDENTIAL_UNAVAILABLE, CredentialRef, ProtocolError, ProtocolErrorClass,
};

use mutsuki_agent_contracts::*;
use mutsuki_agent_runtime::CredentialBrokerService;
use mutsuki_agent_sdk::{
    AgentCredentialImportProtocol, AgentCredentialIssueHandleProtocol,
    AgentCredentialLoginProtocol, AgentCredentialRefreshProtocol, AgentCredentialRevokeProtocol,
    AgentCredentialStatusProtocol, orchestration_runner, service_result_event,
    unsupported_protocol,
};
use mutsuki_runtime_sdk::contracts::{RunnerResult, Task};
use mutsuki_runtime_sdk::{PluginBuilder, RuntimeClientRef, RuntimeResult, TaskAwaitRunnerAdapter};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.credential";
pub const RUNNER_ID: &str = "mutsuki.agent.credential.runner";

pub fn plugin(client: RuntimeClientRef, broker: CredentialBrokerService) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentCredentialLoginProtocol>()
        .protocol::<AgentCredentialImportProtocol>()
        .protocol::<AgentCredentialRefreshProtocol>()
        .protocol::<AgentCredentialRevokeProtocol>()
        .protocol::<AgentCredentialStatusProtocol>()
        .protocol::<AgentCredentialIssueHandleProtocol>()
        .runner(Box::new(runner(client, broker)))
}

pub fn runner(client: RuntimeClientRef, broker: CredentialBrokerService) -> TaskAwaitRunnerAdapter {
    let descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentCredentialLoginProtocol>()
        .accepts::<AgentCredentialImportProtocol>()
        .accepts::<AgentCredentialRefreshProtocol>()
        .accepts::<AgentCredentialRevokeProtocol>()
        .accepts::<AgentCredentialStatusProtocol>()
        .accepts::<AgentCredentialIssueHandleProtocol>()
        .build();
    TaskAwaitRunnerAdapter::new(
        descriptor,
        client,
        Box::new(move |_ctx, task| {
            let broker = broker.clone();
            Box::pin(async move { run_task(broker, task).await })
        }),
    )
}

async fn run_task(broker: CredentialBrokerService, task: Task) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        AGENT_CREDENTIAL_LOGIN_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.credential.login",
            |request: CredentialLoginRequest| broker.login(request),
        ),
        AGENT_CREDENTIAL_IMPORT_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.credential.import",
            |request: CredentialImportRequest| broker.import(request),
        ),
        AGENT_CREDENTIAL_REFRESH_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.credential.refresh",
            |request: CredentialRefreshRequest| broker.refresh(request),
        ),
        AGENT_CREDENTIAL_REVOKE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.credential.revoke",
            |request: CredentialRevokeRequest| broker.revoke(request),
        ),
        AGENT_CREDENTIAL_STATUS_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.credential.status",
            |request: CredentialStatusRequest| broker.status(request),
        ),
        AGENT_CREDENTIAL_ISSUE_HANDLE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.credential.issue_handle",
            |request: CredentialIssueHandleRequest| broker.issue_handle(request),
        ),
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}

/// Bridges runtime CredentialBrokerService into the adapter-facing CredentialBroker trait.
#[derive(Clone)]
pub struct AdapterCredentialBroker {
    service: CredentialBrokerService,
}

impl AdapterCredentialBroker {
    pub fn new(service: CredentialBrokerService) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &CredentialBrokerService {
        &self.service
    }
}

impl CredentialBroker for AdapterCredentialBroker {
    fn resolve(&self, credential: CredentialRef) -> CredentialFuture {
        let service = self.service.clone();
        Box::pin(async move {
            match service.resolve_secret(&credential) {
                Ok(secret) => CredentialValue::new(secret).map_err(|error| ProtocolError {
                    code: error.code.clone(),
                    class: ProtocolErrorClass::Authentication,
                    message: error.message,
                    retry_after_ms: None,
                }),
                Err(error) => Err(ProtocolError {
                    code: if error.code.is_empty() {
                        CREDENTIAL_UNAVAILABLE.into()
                    } else {
                        error.code
                    },
                    class: ProtocolErrorClass::Authentication,
                    message: error.message,
                    retry_after_ms: None,
                }),
            }
        })
    }
}

pub fn adapter_broker(service: CredentialBrokerService) -> Arc<dyn CredentialBroker> {
    Arc::new(AdapterCredentialBroker::new(service))
}
