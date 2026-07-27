use std::{thread, time::Duration};

use mutsuki_agent_protocol::{
    AgentError, AgentMessage, AgentModelGenerateRequest, AgentModelGenerateResult,
    AgentModelStopReason, AgentResult, AgentRole, AgentToolCall, AgentToolDescriptor,
    AgentToolExecuteRequest, AgentToolExecuteResult, AgentUsage, ToolSideEffect,
};
use mutsuki_plugin_agent_model_gateway::ModelProvider;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const BENCHMARK_FIXTURE_VERSION: &str = "mutsuki.agent.benchmark-fixtures/v1";
pub const BENCHMARK_FIXED_SEED: u64 = 1_297_435_713;
pub const BENCHMARK_MODEL_ID: &str = "benchmark-v1";
pub const BENCHMARK_TOOL_NAME: &str = "benchmark-echo-v1";
pub const BENCHMARK_TOOL_PROTOCOL: &str = "mutsuki.agent.benchmark/tool@1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SimulatedLatency {
    ZeroUs,
    OneMs,
    TenMs,
}

impl SimulatedLatency {
    pub const ALL: [Self; 3] = [Self::ZeroUs, Self::OneMs, Self::TenMs];

    pub const fn micros(self) -> u64 {
        match self {
            Self::ZeroUs => 0,
            Self::OneMs => 1_000,
            Self::TenMs => 10_000,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ZeroUs => "0us",
            Self::OneMs => "1ms",
            Self::TenMs => "10ms",
        }
    }

    fn simulate(self) {
        let micros = self.micros();
        if micros > 0 {
            thread::sleep(Duration::from_micros(micros));
        }
    }
}

pub struct BenchmarkModelProvider {
    latency: SimulatedLatency,
}

impl BenchmarkModelProvider {
    pub const fn new(latency: SimulatedLatency) -> Self {
        Self { latency }
    }
}

impl ModelProvider for BenchmarkModelProvider {
    fn provider_id(&self) -> &str {
        BENCHMARK_MODEL_ID
    }

    fn generate(
        &self,
        request: AgentModelGenerateRequest,
    ) -> AgentResult<AgentModelGenerateResult> {
        self.latency.simulate();
        let scenario = metadata_string(&request.metadata, "scenario").unwrap_or("single-turn");
        let failure = metadata_string(&request.metadata, "failure").unwrap_or("none");
        match failure {
            "retryable" => {
                return Err(AgentError::provider_unavailable(
                    "benchmark deterministic retryable failure",
                ));
            }
            "non-retryable" => {
                return Err(AgentError::invalid_input(
                    "benchmark deterministic non-retryable failure",
                ));
            }
            "none" => {}
            other => {
                return Err(AgentError::invalid_input(format!(
                    "unknown benchmark failure mode `{other}`"
                )));
            }
        }

        let completed_tools = request
            .messages
            .iter()
            .filter(|message| message.role == AgentRole::Tool)
            .count();
        let tool_calls = match scenario {
            "single-turn" => Vec::new(),
            "tool-once" if completed_tools == 0 => vec![tool_call(0)],
            "tool-chain-8" if completed_tools < 8 => vec![tool_call(completed_tools)],
            "parallel-tools-8" if completed_tools == 0 => (0..8).map(tool_call).collect(),
            "tool-once" | "tool-chain-8" | "parallel-tools-8" => Vec::new(),
            other => {
                return Err(AgentError::invalid_input(format!(
                    "unknown benchmark scenario `{other}`"
                )));
            }
        };
        let input_tokens = request
            .messages
            .iter()
            .map(|message| message.content.len() as u64)
            .sum::<u64>();
        let content =
            format!("benchmark-v1:{scenario}:seed={BENCHMARK_FIXED_SEED}:tools={completed_tools}");
        let output_tokens = content.len() as u64;
        Ok(AgentModelGenerateResult {
            message: AgentMessage::assistant(content),
            stop_reason: if tool_calls.is_empty() {
                AgentModelStopReason::Stop
            } else {
                AgentModelStopReason::ToolCalls
            },
            tool_calls,
            usage: AgentUsage {
                input_tokens,
                output_tokens,
                total_tokens: input_tokens.saturating_add(output_tokens),
            },
            cost_microunits: 0,
            raw: Some(json!({
                "fixture_version": BENCHMARK_FIXTURE_VERSION,
                "seed": BENCHMARK_FIXED_SEED,
                "latency": self.latency.label()
            })),
            output_resource: None,
        })
    }
}

pub fn benchmark_tool_descriptor() -> AgentToolDescriptor {
    let mut descriptor = AgentToolDescriptor::new(
        BENCHMARK_TOOL_NAME,
        BENCHMARK_TOOL_PROTOCOL,
        "Deterministic benchmark-only pure tool",
    );
    descriptor.side_effect = ToolSideEffect::None;
    descriptor.input_schema = json!({"type": "object", "required": ["index", "seed"]});
    descriptor.output_schema = json!({"type": "object", "required": ["index", "seed"]});
    descriptor
}

pub fn execute_benchmark_tool(
    request: AgentToolExecuteRequest,
    latency: SimulatedLatency,
) -> AgentToolExecuteResult {
    latency.simulate();
    AgentToolExecuteResult {
        call_id: request.call_id,
        name: request.name,
        output: Some(json!({
            "fixture_version": BENCHMARK_FIXTURE_VERSION,
            "seed": BENCHMARK_FIXED_SEED,
            "input": request.input
        })),
        output_ref: None,
        approved: true,
    }
}

fn metadata_string<'a>(metadata: &'a Option<Value>, key: &str) -> Option<&'a str> {
    metadata.as_ref()?.get(key)?.as_str()
}

fn tool_call(index: usize) -> AgentToolCall {
    AgentToolCall {
        call_id: format!("benchmark-call-{index:02}"),
        name: BENCHMARK_TOOL_NAME.into(),
        input: json!({"index": index, "seed": BENCHMARK_FIXED_SEED}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(scenario: &str, tool_messages: usize) -> AgentModelGenerateRequest {
        let mut messages = vec![AgentMessage::user("fixed prompt")];
        messages.extend((0..tool_messages).map(|index| AgentMessage {
            role: AgentRole::Tool,
            content: format!("tool-{index}"),
            name: Some(BENCHMARK_TOOL_NAME.into()),
            metadata: None,
        }));
        AgentModelGenerateRequest {
            model: BENCHMARK_MODEL_ID.into(),
            messages,
            temperature: None,
            max_output_tokens: None,
            provider_hint: None,
            metadata: Some(json!({"scenario": scenario})),
            result_protocol_id: None,
            result_context: None,
            session_id: None,
        }
    }

    #[test]
    fn benchmark_model_produces_stable_tool_scenarios() {
        let provider = BenchmarkModelProvider::new(SimulatedLatency::ZeroUs);
        let first = provider.generate(request("parallel-tools-8", 0)).unwrap();
        let repeated = provider.generate(request("parallel-tools-8", 0)).unwrap();
        assert_eq!(first, repeated);
        assert_eq!(first.tool_calls.len(), 8);
        let final_result = provider.generate(request("parallel-tools-8", 8)).unwrap();
        assert!(final_result.tool_calls.is_empty());
        assert_eq!(final_result.stop_reason, AgentModelStopReason::Stop);
    }

    #[test]
    fn benchmark_failures_keep_retryability_distinct() {
        let provider = BenchmarkModelProvider::new(SimulatedLatency::ZeroUs);
        let mut retryable = request("single-turn", 0);
        retryable.metadata = Some(json!({"scenario": "single-turn", "failure": "retryable"}));
        assert_eq!(
            provider.generate(retryable).unwrap_err().code,
            "agent.provider_unavailable"
        );
        let mut fatal = request("single-turn", 0);
        fatal.metadata = Some(json!({"scenario": "single-turn", "failure": "non-retryable"}));
        assert_eq!(
            provider.generate(fatal).unwrap_err().code,
            "agent.invalid_input"
        );
    }
}
