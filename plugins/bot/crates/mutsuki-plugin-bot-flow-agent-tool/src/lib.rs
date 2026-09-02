//! Approval-gated Agent tools for the Bot Flow document.
//!
//! `bot.flow.read`, `bot.flow.validate` and `bot.flow.apply` reuse the same
//! `mutsuki.bot.flow` `ConfigService` provider as the web flow editor: reads see
//! the active document, validates run the provider validator, and applies go
//! through one revision-CAS activation. The target keeps the neutral Agent
//! execution envelope (`ToolTargetPayloadMode::ExecutionRequest`), so this
//! descriptor surface alone defines the model-facing contract; approval and
//! read-only enforcement stay in the Agent loop and tool router.
#![forbid(unsafe_code)]

use std::sync::Arc;

use mutsuki_agent_contracts::{
    AgentError, AgentToolDescriptor, AgentToolExecuteRequest, ToolSideEffect, ToolTargetPayloadMode,
};
use mutsuki_agent_sdk::{orchestration_runner, runtime_failure};
use mutsuki_bot_flow::{BOT_FLOW_CONFIG_PROVIDER_ID, BotFlowConfigProvider, BotFlowRegistry};
use mutsuki_bot_protocol::{
    BOT_FLOW_TOOL_APPLY, BOT_FLOW_TOOL_PROTOCOL_ID, BOT_FLOW_TOOL_READ, BOT_FLOW_TOOL_VALIDATE,
    BotFlowDocument, BotFlowToolApplyInput, BotFlowToolApplyOutput, BotFlowToolIssue,
    BotFlowToolIssueSeverity, BotFlowToolReadOutput, BotFlowToolValidateInput,
    BotFlowToolValidateOutput,
};
use mutsuki_config_service::{
    ConfigApplyRequest, ConfigContext, ConfigError, ConfigRevision, ConfigService, ValidationIssue,
    ValidationSeverity, capability,
};
use mutsuki_runtime_contracts::{PluginManifest, RunnerDescriptor};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    BoxedTaskAwaitRunner, PluginBuilder, ProtocolSpec, RuntimeClientRef, SdkProtocol,
    TaskAwaitRunnerAdapter,
    contracts::{RunnerResult, Task},
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.plugin.bot.flow.tool";
pub const RUNNER_ID: &str = "mutsuki.agent.bot-flow-tool.runner";

const CAPABILITY_FLOW_READ: &str = "bot.flow.read";
const CAPABILITY_FLOW_WRITE: &str = "bot.flow.write";

const ERR_FLOW_TOOL_REQUEST_INVALID: &str = "bot.flow.tool.request_invalid";
const ERR_FLOW_TOOL_UNKNOWN: &str = "bot.flow.tool.unknown";
const ERR_FLOW_TOOL_REVISION_CONFLICT: &str = "bot.flow.revision_conflict";
const ERR_FLOW_TOOL_VALIDATION_FAILED: &str = "bot.flow.validation_failed";
const ERR_FLOW_TOOL_CONFIG: &str = "bot.flow.config_error";

/// Model-visible business failure of one flow tool op. The runner wraps it as
/// a structured task failure so the tool router surfaces it in
/// `AgentToolExecuteResult.error` and the model can correct and retry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FlowToolError {
    code: &'static str,
    message: String,
}

impl FlowToolError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Marks the single target protocol dispatched by [`flow_tool_runner`].
struct BotFlowToolProtocol;

impl SdkProtocol for BotFlowToolProtocol {
    const PROTOCOL_ID: &'static str = BOT_FLOW_TOOL_PROTOCOL_ID;
}

impl ProtocolSpec for BotFlowToolProtocol {}

/// The model-facing tool set: one read, one validate and one approval-gated
/// apply, all routed through [`BOT_FLOW_TOOL_PROTOCOL_ID`].
#[must_use]
pub fn flow_tool_descriptors() -> Vec<AgentToolDescriptor> {
    vec![
        tool_descriptor(
            BOT_FLOW_TOOL_READ,
            "Read the active Bot Flow document with its revision and the node catalog",
            ToolSideEffect::ExternalRead,
            false,
            json!({"type": "object", "properties": {}, "additionalProperties": false}),
            json!({
                "type": "object",
                "required": ["revision", "flow", "catalog"],
                "properties": {
                    "revision": {"type": "integer", "minimum": 0},
                    "flow": {"type": "object"},
                    "catalog": {"type": "array"}
                }
            }),
            vec![CAPABILITY_FLOW_READ],
        ),
        tool_descriptor(
            BOT_FLOW_TOOL_VALIDATE,
            "Validate a candidate Bot Flow document without applying it",
            ToolSideEffect::ExternalRead,
            false,
            json!({
                "type": "object",
                "required": ["flow"],
                "properties": {"flow": {"type": "object"}},
                "additionalProperties": false
            }),
            json!({
                "type": "object",
                "required": ["ok", "issues"],
                "properties": {
                    "ok": {"type": "boolean"},
                    "issues": {"type": "array"}
                }
            }),
            vec![CAPABILITY_FLOW_WRITE],
        ),
        tool_descriptor(
            BOT_FLOW_TOOL_APPLY,
            "Apply a candidate Bot Flow document with optimistic revision CAS; \
             a stale expected revision is reported as a conflict so the caller \
             can re-read and retry",
            ToolSideEffect::ExternalWrite,
            true,
            json!({
                "type": "object",
                "required": ["flow", "expected_revision"],
                "properties": {
                    "flow": {"type": "object"},
                    "expected_revision": {"type": "integer", "minimum": 0}
                },
                "additionalProperties": false
            }),
            json!({
                "type": "object",
                "required": ["revision", "applied"],
                "properties": {
                    "revision": {"type": "integer", "minimum": 1},
                    "applied": {"type": "boolean"}
                }
            }),
            vec![CAPABILITY_FLOW_WRITE],
        ),
    ]
}

fn tool_descriptor(
    name: &'static str,
    description: &str,
    side_effect: ToolSideEffect,
    requires_approval: bool,
    input_schema: Value,
    output_schema: Value,
    permissions: Vec<&'static str>,
) -> AgentToolDescriptor {
    let mut descriptor = AgentToolDescriptor::new(name, BOT_FLOW_TOOL_PROTOCOL_ID, description);
    descriptor.side_effect = side_effect;
    descriptor.requires_approval = requires_approval;
    descriptor.target_payload_mode = ToolTargetPayloadMode::ExecutionRequest;
    descriptor.input_schema = input_schema;
    descriptor.output_schema = output_schema;
    descriptor.permissions = permissions.into_iter().map(String::from).collect();
    descriptor
}

#[must_use]
fn flow_tool_runner_descriptor() -> RunnerDescriptor {
    orchestration_runner(RUNNER_ID, PLUGIN_ID)
        .accepts::<BotFlowToolProtocol>()
        .build()
}

/// The builtin plugin manifest for the flow tool target: declares the protocol
/// plus the flow capabilities its ops act through.
#[must_use]
pub fn flow_tool_manifest() -> PluginManifest {
    let mut manifest = PluginBuilder::new(PLUGIN_ID)
        .protocol::<BotFlowToolProtocol>()
        .runner_descriptor(flow_tool_runner_descriptor())
        .build()
        .manifest;
    manifest
        .provides
        .capabilities
        .push(CAPABILITY_FLOW_READ.into());
    manifest
        .provides
        .capabilities
        .push(CAPABILITY_FLOW_WRITE.into());
    manifest
}

/// Builds the `mutsuki.bot.flow.tool/execute@1` target runner. The captured
/// `ConfigService` and `BotFlowRegistry` must be the shared instances the flow
/// router activated, so reads see the live graph and applies swap it.
pub fn flow_tool_runner(
    client: RuntimeClientRef,
    config: Arc<ConfigService>,
    registry: Arc<BotFlowRegistry>,
) -> Box<dyn Runner> {
    let descriptor = flow_tool_runner_descriptor();
    let factory: BoxedTaskAwaitRunner = Box::new(move |_ctx, task| {
        let config = config.clone();
        let registry = registry.clone();
        Box::pin(run_flow_tool(config, registry, task))
    });
    Box::new(TaskAwaitRunnerAdapter::new(descriptor, client, factory).with_self_call_policy(false))
}

async fn run_flow_tool(
    config: Arc<ConfigService>,
    registry: Arc<BotFlowRegistry>,
    task: Task,
) -> RuntimeResult<RunnerResult> {
    let request: AgentToolExecuteRequest = serde_json::from_value(task.payload.clone().into())
        .map_err(|error| {
            runtime_failure(
                PLUGIN_ID,
                &task.task_id,
                AgentError::new(
                    ERR_FLOW_TOOL_REQUEST_INVALID,
                    format!("invalid tool execution envelope: {error}"),
                ),
            )
        })?;
    match dispatch_tool(&config, &registry, &request).await {
        Ok(output) => {
            let mut result = RunnerResult::completed(task.task_id);
            result.output = Some(output);
            Ok(result)
        }
        Err(error) => Err(runtime_failure(
            PLUGIN_ID,
            &task.task_id,
            AgentError::new(error.code, error.message),
        )),
    }
}

async fn dispatch_tool(
    config: &ConfigService,
    registry: &BotFlowRegistry,
    request: &AgentToolExecuteRequest,
) -> Result<Value, FlowToolError> {
    match request.name.as_str() {
        BOT_FLOW_TOOL_READ => {
            let output = read_flow(config, registry).await?;
            serde_json::to_value(output).map_err(encode_failure)
        }
        BOT_FLOW_TOOL_VALIDATE => {
            let input: BotFlowToolValidateInput = parse_input(&request.input)?;
            let output = validate_flow(config, &input.flow).await?;
            serde_json::to_value(output).map_err(encode_failure)
        }
        BOT_FLOW_TOOL_APPLY => {
            let input: BotFlowToolApplyInput = parse_input(&request.input)?;
            let output = apply_flow(config, input).await?;
            serde_json::to_value(output).map_err(encode_failure)
        }
        other => Err(FlowToolError::new(
            ERR_FLOW_TOOL_UNKNOWN,
            format!("`{other}` is not a Bot Flow tool"),
        )),
    }
}

async fn read_flow(
    config: &ConfigService,
    registry: &BotFlowRegistry,
) -> Result<BotFlowToolReadOutput, FlowToolError> {
    let snapshot = config
        .read(
            BOT_FLOW_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .map_err(config_error)?;
    let flow = BotFlowConfigProvider::decode(&snapshot.value).map_err(config_error)?;
    Ok(BotFlowToolReadOutput {
        revision: snapshot.revision.0,
        flow,
        catalog: registry.catalog(),
    })
}

async fn validate_flow(
    config: &ConfigService,
    flow: &BotFlowDocument,
) -> Result<BotFlowToolValidateOutput, FlowToolError> {
    let candidate = BotFlowConfigProvider::encode(flow).map_err(config_error)?;
    let result = config
        .validate(
            BOT_FLOW_CONFIG_PROVIDER_ID,
            candidate,
            ConfigContext::global(),
            &[capability::VALUE_WRITE.into()],
        )
        .await
        .map_err(config_error)?;
    Ok(BotFlowToolValidateOutput {
        ok: result.ok,
        issues: result.issues.into_iter().map(tool_issue).collect(),
    })
}

async fn apply_flow(
    config: &ConfigService,
    input: BotFlowToolApplyInput,
) -> Result<BotFlowToolApplyOutput, FlowToolError> {
    let candidate = BotFlowConfigProvider::encode(&input.flow).map_err(config_error)?;
    let result = config
        .apply(
            BOT_FLOW_CONFIG_PROVIDER_ID,
            ConfigApplyRequest {
                candidate,
                expected_revision: ConfigRevision(input.expected_revision),
                dry_run: false,
            },
            ConfigContext::global(),
            &[capability::VALUE_WRITE.into(), capability::APPLY.into()],
        )
        .await
        .map_err(|error| match error {
            ConfigError::RevisionConflict {
                expected, current, ..
            } => FlowToolError::new(
                ERR_FLOW_TOOL_REVISION_CONFLICT,
                format!("expected revision {expected}, current revision is {current}; read the flow again and retry"),
            ),
            ConfigError::ValidationFailed { result } => FlowToolError::new(
                ERR_FLOW_TOOL_VALIDATION_FAILED,
                validation_summary(&result.issues),
            ),
            other => config_error(other),
        })?;
    Ok(BotFlowToolApplyOutput {
        revision: result.revision.0,
        applied: result.applied,
    })
}

fn parse_input<T: serde::de::DeserializeOwned>(input: &Value) -> Result<T, FlowToolError> {
    serde_json::from_value(input.clone()).map_err(|error| {
        FlowToolError::new(
            ERR_FLOW_TOOL_REQUEST_INVALID,
            format!("invalid tool input: {error}"),
        )
    })
}

#[allow(clippy::needless_pass_by_value)]
fn config_error(error: ConfigError) -> FlowToolError {
    FlowToolError::new(ERR_FLOW_TOOL_CONFIG, error.to_string())
}

#[allow(clippy::needless_pass_by_value)]
fn encode_failure(error: serde_json::Error) -> FlowToolError {
    FlowToolError::new(ERR_FLOW_TOOL_CONFIG, error.to_string())
}

fn tool_issue(issue: ValidationIssue) -> BotFlowToolIssue {
    BotFlowToolIssue {
        path: issue.path.0,
        severity: match issue.severity {
            ValidationSeverity::Error => BotFlowToolIssueSeverity::Error,
            ValidationSeverity::Warning => BotFlowToolIssueSeverity::Warning,
        },
        message: issue.message.default,
    }
}

fn validation_summary(issues: &[ValidationIssue]) -> String {
    if issues.is_empty() {
        return "candidate was rejected without issues".into();
    }
    issues
        .iter()
        .map(|issue| {
            let path = issue.path.0.join(".");
            format!(
                "{}: {}",
                if path.is_empty() { "<root>" } else { &path },
                issue.message.default
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_executor::block_on;
    use mutsuki_agent_contracts::AgentPermissionMode;
    use mutsuki_bot_flow::BotNodeCatalog;
    use mutsuki_bot_protocol::{BotFlowEdge, BotFlowNode, BotFlowNodePosition};
    use mutsuki_config_service::{ConfigProviderRegistry, InMemoryConfigRepository};
    use mutsuki_runtime_sdk::contracts::ScalarValue;

    fn service_with_flow() -> (Arc<ConfigService>, Arc<BotFlowRegistry>) {
        let flow = Arc::new(BotFlowRegistry::new(BotNodeCatalog::default()));
        let providers = Arc::new(ConfigProviderRegistry::default());
        providers
            .register(Arc::new(BotFlowConfigProvider::new(flow.clone())))
            .unwrap();
        let service = Arc::new(
            ConfigService::new(providers, Arc::new(InMemoryConfigRepository::default())).unwrap(),
        );
        (service, flow)
    }

    fn execute_request(name: &str, input: Value) -> AgentToolExecuteRequest {
        AgentToolExecuteRequest {
            call_id: Some("call-1".into()),
            name: name.into(),
            input,
            session_id: Some("session".into()),
            approval: None,
            context: None,
            permission_mode: AgentPermissionMode::Ask,
        }
    }

    fn sample_flow() -> BotFlowDocument {
        BotFlowDocument {
            nodes: vec![BotFlowNode {
                node_id: "n1".into(),
                node_type_id: "mutsuki.bot.node/agent-submit".into(),
                node_type_version: 1,
                config: json!({}),
                source: None,
                position: BotFlowNodePosition::default(),
            }],
            edges: vec![BotFlowEdge {
                edge_id: "e1".into(),
                from_node_id: "n1".into(),
                from_port_id: "out".into(),
                to_node_id: "n1".into(),
                to_port_id: "in".into(),
                kind: mutsuki_bot_protocol::BotFlowEdgeKind::Event,
            }],
            ..BotFlowDocument::default()
        }
    }

    #[test]
    fn descriptors_route_through_one_protocol_and_gate_apply() {
        let descriptors = flow_tool_descriptors();
        let names: Vec<&str> = descriptors.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            [
                BOT_FLOW_TOOL_READ,
                BOT_FLOW_TOOL_VALIDATE,
                BOT_FLOW_TOOL_APPLY
            ]
        );
        for descriptor in &descriptors {
            assert_eq!(descriptor.target_protocol_id, BOT_FLOW_TOOL_PROTOCOL_ID);
            assert_eq!(
                descriptor.target_payload_mode,
                ToolTargetPayloadMode::ExecutionRequest
            );
        }
        let read = &descriptors[0];
        assert_eq!(read.side_effect, ToolSideEffect::ExternalRead);
        assert!(!read.requires_approval);
        assert_eq!(read.permissions, vec![CAPABILITY_FLOW_READ.to_string()]);

        let apply = &descriptors[2];
        assert_eq!(apply.side_effect, ToolSideEffect::ExternalWrite);
        assert!(apply.requires_approval);
        assert_eq!(apply.permissions, vec![CAPABILITY_FLOW_WRITE.to_string()]);
    }

    #[test]
    fn manifest_declares_tool_protocol_and_flow_capabilities() {
        let manifest = flow_tool_manifest();
        assert_eq!(manifest.plugin_id, PLUGIN_ID);
        assert!(
            manifest
                .provides
                .protocols
                .iter()
                .any(|protocol| protocol.protocol_id == BOT_FLOW_TOOL_PROTOCOL_ID)
        );
        assert!(
            manifest
                .provides
                .capabilities
                .contains(&CAPABILITY_FLOW_READ.into())
        );
        assert!(
            manifest
                .provides
                .capabilities
                .contains(&CAPABILITY_FLOW_WRITE.into())
        );
    }

    #[test]
    fn read_returns_active_document_with_revision_and_catalog() {
        let (config, registry) = service_with_flow();
        let output = block_on(read_flow(&config, &registry)).unwrap();
        assert_eq!(output.revision, ConfigRevision::ABSENT.0);
        assert_eq!(output.flow, BotFlowDocument::default());
        assert!(output.catalog.is_empty());
    }

    #[test]
    fn validate_surfaces_provider_issues_without_applying() {
        let (config, registry) = service_with_flow();
        let flow = sample_flow();
        let output = block_on(validate_flow(&config, &flow)).unwrap();
        assert!(!output.ok);
        assert!(!output.issues.is_empty());
        assert_eq!(registry.active().revision, 0);
    }

    #[test]
    fn apply_is_one_revision_cas_and_conflict_is_a_business_error() {
        let (config, registry) = service_with_flow();
        let input = BotFlowToolApplyInput {
            flow: BotFlowDocument::default(),
            expected_revision: 0,
        };
        let output = block_on(apply_flow(&config, input.clone())).unwrap();
        assert!(output.applied);
        assert_eq!(output.revision, 1);
        assert_eq!(registry.active().revision, 1);

        let error = block_on(apply_flow(&config, input)).unwrap_err();
        assert_eq!(error.code, ERR_FLOW_TOOL_REVISION_CONFLICT);
        assert!(error.message.contains("current revision is 1"));
        assert_eq!(registry.active().revision, 1);
    }

    #[test]
    fn dispatch_reports_unknown_tools_and_invalid_input() {
        let (config, registry) = service_with_flow();
        let error = block_on(dispatch_tool(
            &config,
            &registry,
            &execute_request("bot.flow.nonsense", json!({})),
        ))
        .unwrap_err();
        assert_eq!(error.code, ERR_FLOW_TOOL_UNKNOWN);

        let error = block_on(dispatch_tool(
            &config,
            &registry,
            &execute_request(BOT_FLOW_TOOL_APPLY, json!({ "flow": {} })),
        ))
        .unwrap_err();
        assert_eq!(error.code, ERR_FLOW_TOOL_REQUEST_INVALID);
    }

    #[test]
    fn runner_wraps_outputs_and_business_failures_for_the_router() {
        let (config, registry) = service_with_flow();
        let read = Task::new(
            "flow-tool-read",
            BOT_FLOW_TOOL_PROTOCOL_ID,
            serde_json::to_value(execute_request(BOT_FLOW_TOOL_READ, json!({}))).unwrap(),
        );
        let result = block_on(run_flow_tool(config.clone(), registry.clone(), read)).unwrap();
        let output = result.output.expect("read output");
        assert_eq!(output["flow"]["flow_id"], "default");

        let conflict = Task::new(
            "flow-tool-apply",
            BOT_FLOW_TOOL_PROTOCOL_ID,
            serde_json::to_value(execute_request(
                BOT_FLOW_TOOL_APPLY,
                json!({"flow": {}, "expected_revision": 5}),
            ))
            .unwrap(),
        );
        let error = block_on(run_flow_tool(config, registry, conflict)).unwrap_err();
        assert_eq!(error.0.code, ERR_FLOW_TOOL_REVISION_CONFLICT);
        assert!(
            error
                .0
                .evidence
                .get("message")
                .is_some_and(|message| matches!(message, ScalarValue::String(body) if body.contains("current revision is 0")))
        );
    }
}
