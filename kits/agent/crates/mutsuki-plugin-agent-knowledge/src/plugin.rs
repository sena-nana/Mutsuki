use mutsuki_agent_contracts::*;
use mutsuki_agent_runtime::KnowledgeService;
use mutsuki_agent_sdk::{
    AgentKnowledgeDeleteProtocol, AgentKnowledgeIngestProtocol, AgentKnowledgeRetrieveProtocol,
    orchestration_runner, service_result_event, unsupported_protocol,
};
use mutsuki_runtime_sdk::contracts::{RunnerResult, Task};
use mutsuki_runtime_sdk::{PluginBuilder, RuntimeClientRef, RuntimeResult, TaskAwaitRunnerAdapter};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.knowledge";
pub const RUNNER_ID: &str = "mutsuki.agent.knowledge.runner";

pub fn plugin(client: RuntimeClientRef, service: KnowledgeService) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentKnowledgeIngestProtocol>()
        .protocol::<AgentKnowledgeRetrieveProtocol>()
        .protocol::<AgentKnowledgeDeleteProtocol>()
        .runner(Box::new(runner(client, service)))
}

pub fn runner(client: RuntimeClientRef, service: KnowledgeService) -> TaskAwaitRunnerAdapter {
    let descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentKnowledgeIngestProtocol>()
        .accepts::<AgentKnowledgeRetrieveProtocol>()
        .accepts::<AgentKnowledgeDeleteProtocol>()
        .build();
    TaskAwaitRunnerAdapter::new(
        descriptor,
        client,
        Box::new(move |_ctx, task| {
            let service = service.clone();
            Box::pin(async move { run_task(service, task).await })
        }),
    )
}

async fn run_task(service: KnowledgeService, task: Task) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        AGENT_KNOWLEDGE_INGEST_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.knowledge.ingested",
            |request: IngestionPlan| service.ingest(request),
        ),
        AGENT_KNOWLEDGE_RETRIEVE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.knowledge.retrieved",
            |request: RetrievalQuery| service.retrieve(request),
        ),
        AGENT_KNOWLEDGE_DELETE_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.knowledge.deleted",
            |request: KnowledgeDeleteRequest| service.delete(request),
        ),
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}
