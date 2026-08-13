use crate::ToolRegistry;
use mutsuki_agent_contracts::*;
use mutsuki_agent_sdk::{
    AgentToolExecuteProtocol, AgentToolListProtocol, orchestration_runner, result_event,
    runtime_failure, service_result_event, task_payload, unsupported_protocol,
};
use mutsuki_runtime_sdk::contracts::{RunnerResult, ScalarValue, Task, TaskOutcome};
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, PluginBuilder, RuntimeClientRef, RuntimeResult, TaskAwaitRunnerAdapter,
};

pub const PLUGIN_ID: &str = "mutsuki.plugin.agent.tool_router";
pub const RUNNER_ID: &str = "mutsuki.agent.tool_router.runner";

pub fn plugin(client: RuntimeClientRef, registry: ToolRegistry) -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .protocol::<AgentToolListProtocol>()
        .protocol::<AgentToolExecuteProtocol>()
        .runner(Box::new(runner(client, registry)))
}

pub fn runner(client: RuntimeClientRef, registry: ToolRegistry) -> TaskAwaitRunnerAdapter {
    let mut descriptor = orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<AgentToolListProtocol>()
        .accepts::<AgentToolExecuteProtocol>();
    for tool in registry.list(AgentToolListRequest::default()).tools {
        if matches!(tool.execution, AgentToolExecution::Routed) {
            descriptor = descriptor.requires_protocol(tool.target_protocol_id);
        }
    }
    let descriptor = descriptor.build();
    TaskAwaitRunnerAdapter::new(
        descriptor,
        client,
        Box::new(move |ctx, task| {
            let registry = registry.clone();
            Box::pin(async move { run_task(registry, ctx, task).await })
        }),
    )
}

async fn run_task(
    registry: ToolRegistry,
    ctx: AsyncRunnerContext,
    task: Task,
) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        AGENT_TOOL_LIST_PROTOCOL => service_result_event(
            PLUGIN_ID,
            &task,
            "mutsuki.agent.tool.listed",
            |request: AgentToolListRequest| Ok(registry.list(request)),
        ),
        AGENT_TOOL_EXECUTE_PROTOCOL => {
            let request: AgentToolExecuteRequest = task_payload(PLUGIN_ID, &task)?;
            let descriptor = registry
                .get(&request.name)
                .map_err(|error| runtime_failure(PLUGIN_ID, &task.task_id, error))?;
            if !matches!(descriptor.execution, AgentToolExecution::Routed) {
                return Err(runtime_failure(
                    PLUGIN_ID,
                    &task.task_id,
                    AgentError::new(
                        "agent.interaction.loop_required",
                        "interaction tools must be handled by the active Agent loop",
                    ),
                ));
            }
            if descriptor.requires_approval {
                validate_approval(&request, &descriptor)
                    .map_err(|error| runtime_failure(PLUGIN_ID, &task.task_id, error))?;
            }
            let payload = target_payload(&descriptor, &request)
                .map_err(|error| runtime_failure(PLUGIN_ID, &task.task_id, error))?;
            let outcome = ctx
                .call_raw(descriptor.target_protocol_id.clone(), payload)
                .await?;
            let result = tool_result(&task, request.call_id, request.name, outcome)?;
            result_event(task.task_id, "mutsuki.agent.tool.executed", result)
        }
        _ => Err(unsupported_protocol(PLUGIN_ID, &task)),
    }
}

fn target_payload(
    descriptor: &AgentToolDescriptor,
    request: &AgentToolExecuteRequest,
) -> AgentResult<serde_json::Value> {
    match descriptor.target_payload_mode {
        ToolTargetPayloadMode::RawInput => Ok(request.input.clone()),
        ToolTargetPayloadMode::ExecutionRequest => serde_json::to_value(request)
            .map_err(|error| AgentError::invalid_input(error.to_string())),
    }
}

fn validate_approval(
    execution: &AgentToolExecuteRequest,
    descriptor: &AgentToolDescriptor,
) -> AgentResult<()> {
    let approval = execution.approval.as_ref().ok_or_else(|| {
        AgentError::new(
            "agent.approval_required",
            format!("tool `{}` requires approval", descriptor.name),
        )
    })?;
    let call_id = execution.call_id.as_deref().unwrap_or_default();
    let session_id = execution.session_id.as_deref().unwrap_or_default();
    let request = &approval.request;
    let decision = &approval.decision;
    if call_id.is_empty()
        || session_id.is_empty()
        || request.session_id != session_id
        || request.action_id != call_id
        || request.tool != descriptor.name
        || decision.session_id != request.session_id
        || decision.turn_id != request.turn_id
        || decision.action_id != request.action_id
        || decision.version != request.version
        || decision.decision != PermissionDecisionKind::Approved
    {
        return Err(AgentError::new(
            "agent.approval.invalid_binding",
            "tool approval must be approved and bind the session, turn, action, version and tool",
        ));
    }
    Ok(())
}

fn tool_result(
    task: &Task,
    call_id: Option<String>,
    name: String,
    outcome: TaskOutcome,
) -> RuntimeResult<AgentToolExecuteResult> {
    match outcome {
        TaskOutcome::Completed {
            output, output_ref, ..
        } if output.is_some() || output_ref.is_some() => Ok(AgentToolExecuteResult {
            call_id,
            name,
            output,
            output_ref,
            error: None,
            approved: true,
        }),
        TaskOutcome::Completed { .. } => Err(runtime_failure(
            PLUGIN_ID,
            &task.task_id,
            AgentError::new(
                "agent.result_missing",
                "tool target completed without a business result",
            ),
        )),
        TaskOutcome::Failed { error, .. } => Ok(AgentToolExecuteResult {
            call_id,
            name,
            output: None,
            output_ref: None,
            error: Some(AgentError::new(
                error.code.clone(),
                match error.evidence.get("message") {
                    Some(ScalarValue::String(message)) => message.clone(),
                    _ => error.route.clone(),
                },
            )),
            approved: true,
        }),
        outcome => Err(runtime_failure(
            PLUGIN_ID,
            &task.task_id,
            AgentError::new(
                "agent.tool.target_failed",
                format!("tool target did not complete: {outcome:?}"),
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_reads_inline_target_output() {
        let task = Task::new("tool-1", AGENT_TOOL_EXECUTE_PROTOCOL, serde_json::json!({}));
        let result = tool_result(
            &task,
            Some("call-1".into()),
            "echo".into(),
            TaskOutcome::Completed {
                task_id: "target-1".into(),
                output: Some(serde_json::json!({"value": "ping"})),
                output_ref: None,
            },
        )
        .unwrap();

        assert_eq!(result.call_id.as_deref(), Some("call-1"));
        assert_eq!(result.output, Some(serde_json::json!({"value": "ping"})));
        assert!(result.output_ref.is_none());
        assert!(result.error.is_none());
    }

    #[test]
    fn tool_result_preserves_provider_owned_output_reference() {
        let task = Task::new("tool-1", AGENT_TOOL_EXECUTE_PROTOCOL, serde_json::json!({}));
        let result = tool_result(
            &task,
            None,
            "large-output".into(),
            TaskOutcome::Completed {
                task_id: "target-1".into(),
                output: None,
                output_ref: Some("resource:tool-output".into()),
            },
        )
        .unwrap();

        assert!(result.output.is_none());
        assert_eq!(result.output_ref.as_deref(), Some("resource:tool-output"));
        assert!(result.error.is_none());
    }

    #[test]
    fn tool_business_failure_is_returned_as_structured_result() {
        let task = Task::new("tool-1", AGENT_TOOL_EXECUTE_PROTOCOL, serde_json::json!({}));
        let mut error = mutsuki_runtime_sdk::contracts::RuntimeError::new(
            "tool.rejected",
            "test-tool",
            "workspace.write",
        );
        error.evidence.insert(
            "message".into(),
            ScalarValue::String("write was rejected".into()),
        );
        let result = tool_result(
            &task,
            Some("call-1".into()),
            "workspace.write".into(),
            TaskOutcome::Failed {
                task_id: "target-1".into(),
                error,
            },
        )
        .unwrap();

        assert_eq!(result.call_id.as_deref(), Some("call-1"));
        assert_eq!(result.error.unwrap().code, "tool.rejected");
    }

    #[test]
    fn approval_is_bound_to_the_exact_tool_action() {
        let descriptor = AgentToolDescriptor {
            requires_approval: true,
            ..AgentToolDescriptor::new("write", "workspace.write@1", "write")
        };
        let request = PermissionRequest {
            session_id: "session".into(),
            turn_id: "turn".into(),
            action_id: "call".into(),
            tool: "write".into(),
            side_effect: ToolSideEffect::WorkspaceWrite,
            summary: "write".into(),
            version: 3,
        };
        let execution = AgentToolExecuteRequest {
            call_id: Some("call".into()),
            name: "write".into(),
            input: serde_json::json!({}),
            session_id: Some("session".into()),
            approval: Some(AgentToolApproval {
                request: request.clone(),
                decision: PermissionDecision {
                    session_id: "session".into(),
                    turn_id: "turn".into(),
                    action_id: "call".into(),
                    version: 3,
                    decision: PermissionDecisionKind::Approved,
                },
            }),
            context: Some(serde_json::json!({"workspace": "/repo"})),
        };
        validate_approval(&execution, &descriptor).unwrap();
        let mut stale = execution;
        stale.approval.as_mut().unwrap().decision.version = 2;
        assert_eq!(
            validate_approval(&stale, &descriptor).unwrap_err().code,
            "agent.approval.invalid_binding"
        );
    }

    #[test]
    fn target_payload_mode_preserves_raw_input_and_opt_in_execution_context() {
        let request = AgentToolExecuteRequest {
            call_id: Some("call".into()),
            name: "echo".into(),
            input: serde_json::json!({"value": "ping"}),
            session_id: Some("session".into()),
            approval: None,
            context: Some(serde_json::json!({"workspace": "/repo"})),
        };
        let raw = AgentToolDescriptor::new("echo", "test.echo@1", "echo");
        assert_eq!(target_payload(&raw, &request).unwrap(), request.input);

        let contextual = AgentToolDescriptor {
            target_payload_mode: ToolTargetPayloadMode::ExecutionRequest,
            ..raw
        };
        let payload = target_payload(&contextual, &request).unwrap();
        assert_eq!(payload["call_id"], "call");
        assert_eq!(payload["context"]["workspace"], "/repo");
    }
}
