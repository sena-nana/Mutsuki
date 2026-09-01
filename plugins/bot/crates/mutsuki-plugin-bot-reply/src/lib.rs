// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use mutsuki_bot_protocol::BotNodeWiring;
use mutsuki_bot_protocol::{
    BOT_EXT_REPLY_SOURCE_MESSAGE_ID, BOT_EXT_REPLY_SOURCE_USER_ID, BOT_FLOW_DELIVERY_REPLY_TYPE,
    BOT_REPLY_MENTION_PROTOCOL_ID, BOT_REPLY_QUOTE_PROTOCOL_ID, BOT_REPLY_SEGMENT_PROTOCOL_ID,
    BotDeliveryContent, BotFlowTypeRef, BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor,
    BotNodeInvocation, BotNodeOutput, BotNodePortDescriptor, BotNodePortDirection, BotNodeResult,
    BotNodeRole, BotReplyDeliveryPart, BotReplyDeliveryRequest, MessageSegment,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, PluginManifest, RunnerDescriptor, RunnerResult, Task,
    WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder, map_work_batch_entries,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

pub const BOT_REPLY_PLUGIN_ID: &str = "mutsuki.plugin.bot.reply";
pub const BOT_REPLY_RUNNER_ID: &str = "mutsuki.bot.reply";
pub const BOT_REPLY_QUOTE_NODE_TYPE: &str = "mutsuki.bot.reply.quote";
pub const BOT_REPLY_MENTION_NODE_TYPE: &str = "mutsuki.bot.reply.mention";
pub const BOT_REPLY_SEGMENT_NODE_TYPE: &str = "mutsuki.bot.reply.segment";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SegmentConfig {
    pattern: String,
    interval_ms: u64,
    words_count_threshold: usize,
}

impl SegmentConfig {
    fn regex(&self) -> Result<Regex, String> {
        let pattern = if self.pattern.trim().is_empty() {
            r".*?[。？！~…]+|.+$"
        } else {
            self.pattern.as_str()
        };
        Regex::new(pattern).map_err(|error| error.to_string())
    }
}

#[must_use]
pub fn bot_reply_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_REPLY_PLUGIN_ID)
        .runner_descriptor(
            RunnerDescriptorBuilder::new(BOT_REPLY_RUNNER_ID, BOT_REPLY_PLUGIN_ID)
                .accepted_protocol(BOT_REPLY_QUOTE_PROTOCOL_ID)
                .accepted_protocol(BOT_REPLY_MENTION_PROTOCOL_ID)
                .accepted_protocol(BOT_REPLY_SEGMENT_PROTOCOL_ID)
                .execution_class(ExecutionClass::Orchestration)
                .build(),
        )
        .protocol_handler(
            node_protocol(BOT_REPLY_QUOTE_PROTOCOL_ID),
            BOT_REPLY_RUNNER_ID,
            "quote",
        )
        .protocol_handler(
            node_protocol(BOT_REPLY_MENTION_PROTOCOL_ID),
            BOT_REPLY_RUNNER_ID,
            "mention",
        )
        .protocol_handler(
            node_protocol(BOT_REPLY_SEGMENT_PROTOCOL_ID),
            BOT_REPLY_RUNNER_ID,
            "segment",
        )
        .extension(
            reply_node_catalog()
                .into_plugin_extension()
                .expect("reply catalog serializes"),
        )
        .build()
        .manifest
}

fn node_protocol(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({"type": "object"}))
        .output_schema(json!({"type": "object", "required": ["outputs", "metadata"]}))
        .error_schema(json!({"type": "object", "required": ["code", "source", "route"]}))
        .build()
}

fn reply_node_catalog() -> BotNodeCatalogFragment {
    BotNodeCatalogFragment {
        nodes: vec![
            reply_node(
                BOT_REPLY_QUOTE_NODE_TYPE,
                "引用原消息",
                BOT_REPLY_QUOTE_PROTOCOL_ID,
                json!({"type": "object", "additionalProperties": false}),
            ),
            reply_node(
                BOT_REPLY_MENTION_NODE_TYPE,
                "@ 用户",
                BOT_REPLY_MENTION_PROTOCOL_ID,
                json!({"type": "object", "additionalProperties": false}),
            ),
            reply_node(
                BOT_REPLY_SEGMENT_NODE_TYPE,
                "分段回复",
                BOT_REPLY_SEGMENT_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "pattern": {"type": "string", "title": "分隔正则", "default": ".*?[。？！~…]+|.+$"},
                        "interval_ms": {"type": "integer", "minimum": 0, "default": 0, "title": "分段间隔毫秒"},
                        "words_count_threshold": {"type": "integer", "minimum": 1, "default": 150, "title": "超过则不分段"}
                    }
                }),
            ),
        ],
    }
}

fn reply_node(
    node_type_id: &str,
    title: &str,
    protocol_id: &str,
    config_schema: Value,
) -> BotNodeDescriptor {
    BotNodeDescriptor {
        node_type_id: node_type_id.into(),
        version: 1,
        title: title.into(),
        category: "呈现".into(),
        role: BotNodeRole::Processor,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{protocol_id}"),
            protocol_id: protocol_id.into(),
            runner_hint: Some(BOT_REPLY_RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "reply".into(),
                title: "回复".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new(BOT_FLOW_DELIVERY_REPLY_TYPE, 1),
                required: true,
            },
            BotNodePortDescriptor {
                port_id: "reply".into(),
                title: "回复".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(BOT_FLOW_DELIVERY_REPLY_TYPE, 1),
                required: false,
            },
        ],
        config_schema,
    }
}

pub struct BotReplyRunner {
    descriptor: RunnerDescriptor,
}

impl Default for BotReplyRunner {
    fn default() -> Self {
        Self {
            descriptor: RunnerDescriptorBuilder::new(BOT_REPLY_RUNNER_ID, BOT_REPLY_PLUGIN_ID)
                .accepted_protocol(BOT_REPLY_QUOTE_PROTOCOL_ID)
                .accepted_protocol(BOT_REPLY_MENTION_PROTOCOL_ID)
                .accepted_protocol(BOT_REPLY_SEGMENT_PROTOCOL_ID)
                .execution_class(ExecutionClass::Orchestration)
                .build(),
        }
    }
}

impl Runner for BotReplyRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let invocation = task
                .payload
                .decode_shared::<BotNodeInvocation>()
                .map_err(|error| runtime_error(task, error))?;
            let mut request: BotReplyDeliveryRequest =
                serde_json::from_value(invocation.input.payload.value.clone())
                    .map_err(|error| runtime_error(task, error))?;
            match task.protocol_id.as_str() {
                BOT_REPLY_QUOTE_PROTOCOL_ID => apply_quote(&invocation, &mut request),
                BOT_REPLY_MENTION_PROTOCOL_ID => apply_mention(&invocation, &mut request),
                BOT_REPLY_SEGMENT_PROTOCOL_ID => {
                    apply_segment(&invocation.config, &mut request)
                        .map_err(|error| runtime_error(task, error))?;
                }
                other => return Err(runtime_error(task, other)),
            }
            completed_reply(task, &invocation, request)
        })
    }
}

fn apply_quote(invocation: &BotNodeInvocation, request: &mut BotReplyDeliveryRequest) {
    let Some(message_id) = context_string(
        &invocation.input.context.ext,
        BOT_EXT_REPLY_SOURCE_MESSAGE_ID,
    ) else {
        return;
    };
    for part in &mut request.parts {
        part.content.reply_to = Some(message_id.clone());
        if !part.content.segments.iter().any(|segment| {
            matches!(
                segment,
                MessageSegment::Quote { .. } | MessageSegment::Reply { .. }
            )
        }) {
            part.content.segments.insert(
                0,
                MessageSegment::Quote {
                    message_id: message_id.clone(),
                },
            );
        }
    }
}

fn apply_mention(invocation: &BotNodeInvocation, request: &mut BotReplyDeliveryRequest) {
    let Some(user_id) = context_string(&invocation.input.context.ext, BOT_EXT_REPLY_SOURCE_USER_ID)
    else {
        return;
    };
    for part in &mut request.parts {
        if !part.content.segments.iter().any(|segment| {
            matches!(segment, MessageSegment::MentionUser { user_id: id } if id == &user_id)
        }) {
            part.content
                .segments
                .insert(0, MessageSegment::MentionUser { user_id: user_id.clone() });
        }
    }
}

fn apply_segment(config: &Value, request: &mut BotReplyDeliveryRequest) -> Result<(), String> {
    let config: SegmentConfig =
        serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
    let regex = config.regex()?;
    let mut parts = Vec::new();
    let mut delay_ms = 0_u64;
    for part in &request.parts {
        let text = part
            .content
            .segments
            .iter()
            .filter_map(|segment| match segment {
                MessageSegment::Text { text } | MessageSegment::Markdown { content: text } => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        if config.words_count_threshold > 0 && text.chars().count() >= config.words_count_threshold
        {
            parts.push(part.clone());
            continue;
        }
        let chunks: Vec<String> = regex
            .find_iter(&text)
            .map(|matched| matched.as_str().trim().to_owned())
            .filter(|chunk| !chunk.is_empty())
            .collect();
        if chunks.len() <= 1 {
            parts.push(part.clone());
            continue;
        }
        for (index, chunk) in chunks.into_iter().enumerate() {
            let mut next = BotReplyDeliveryPart {
                part_id: format!("{}:seg:{index}", part.part_id),
                content: BotDeliveryContent {
                    segments: vec![MessageSegment::text(chunk)],
                    summary: part.content.summary.clone(),
                    reply_to: part.content.reply_to.clone(),
                },
                not_before_unix_ms: None,
            };
            if index > 0 && config.interval_ms > 0 {
                delay_ms = delay_ms.saturating_add(config.interval_ms);
                next.not_before_unix_ms = Some(unix_ms().saturating_add(delay_ms));
            }
            parts.push(next);
        }
    }
    if !parts.is_empty() {
        request.parts = parts;
    }
    Ok(())
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn context_string(ext: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    ext.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn completed_reply(
    task: &Task,
    invocation: &BotNodeInvocation,
    request: BotReplyDeliveryRequest,
) -> Result<RunnerResult, mutsuki_runtime_contracts::RuntimeError> {
    let mut event = invocation.input.clone();
    event.payload.value =
        serde_json::to_value(&request).map_err(|error| runtime_error(task, error))?;
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs: vec![BotNodeOutput {
                port_id: "reply".into(),
                event,
            }],
            metadata: BTreeMap::new(),
        })
        .map_err(|error| runtime_error(task, error))?,
    );
    Ok(result)
}

fn runtime_error(
    task: &Task,
    error: impl std::fmt::Display,
) -> mutsuki_runtime_contracts::RuntimeError {
    let mut value = mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_REPLY_PLUGIN_ID,
        format!("{}.{}", task.task_id, task.protocol_id),
    );
    value.evidence.insert(
        "message".into(),
        mutsuki_runtime_contracts::ScalarValue::String(error.to_string()),
    );
    value
}

#[cfg(test)]
mod tests {
    use mutsuki_bot_protocol::{
        BotConversationKind, BotFlowContext, BotFlowEventEnvelope, BotFlowPayload, DeliveryPolicy,
        QQ_CONVERSATION_REF_VERSION, QqConversationRef,
    };

    use super::*;

    fn request(text: &str) -> BotReplyDeliveryRequest {
        BotReplyDeliveryRequest {
            reply_id: "r1".into(),
            idempotency_key: "r1".into(),
            conversation: QqConversationRef {
                version: QQ_CONVERSATION_REF_VERSION,
                account_id: "bot".into(),
                kind: BotConversationKind::Group,
                user_id: None,
                group_id: Some("g1".into()),
                guild_id: None,
                channel_id: None,
                thread_id: None,
            },
            parts: vec![BotReplyDeliveryPart {
                part_id: "p1".into(),
                content: BotDeliveryContent {
                    segments: vec![MessageSegment::text(text)],
                    summary: None,
                    reply_to: None,
                },
                not_before_unix_ms: None,
            }],
            policy: DeliveryPolicy {
                max_attempts: 1,
                initial_backoff_ms: 1,
                max_backoff_ms: 1,
                not_before_unix_ms: None,
                expires_at_unix_ms: None,
            },
            source_event_id: "e1".into(),
            source_turn_id: "t1".into(),
            source_binding_key: None,
            occupancy_only: false,
        }
    }

    fn invocation(request: &BotReplyDeliveryRequest) -> BotNodeInvocation {
        BotNodeInvocation {
            flow_id: "flow".into(),
            graph_revision: 1,
            execution_id: "ex".into(),
            node_id: "n".into(),
            input_port_id: "reply".into(),
            wiring: BotNodeWiring::default(),
            config: json!({}),
            input: BotFlowEventEnvelope {
                event_id: "e1".into(),
                protocol_id: "mutsuki.bot.delivery/reply@1".into(),
                payload: BotFlowPayload {
                    event_type: BotFlowTypeRef::new(BOT_FLOW_DELIVERY_REPLY_TYPE, 1),
                    value: serde_json::to_value(request).unwrap(),
                },
                context: BotFlowContext {
                    bot: None,
                    target: None,
                    actor: None,
                    ext: BTreeMap::from([
                        (BOT_EXT_REPLY_SOURCE_MESSAGE_ID.into(), json!("m1")),
                        (BOT_EXT_REPLY_SOURCE_USER_ID.into(), json!("u1")),
                    ]),
                },
                trace_id: None,
                correlation_id: None,
            },
        }
    }

    #[test]
    fn quote_and_mention_prepend_segments() {
        let mut request = request("hello");
        let invocation = invocation(&request);
        apply_quote(&invocation, &mut request);
        apply_mention(&invocation, &mut request);
        assert_eq!(request.parts[0].content.reply_to.as_deref(), Some("m1"));
        assert!(matches!(
            request.parts[0].content.segments[0],
            MessageSegment::MentionUser { .. }
        ));
        assert!(matches!(
            request.parts[0].content.segments[1],
            MessageSegment::Quote { .. }
        ));
    }

    #[test]
    fn segment_splits_sentences() {
        let mut request = request("你好。世界！");
        apply_segment(
            &json!({
                "pattern": "",
                "interval_ms": 0,
                "words_count_threshold": 150
            }),
            &mut request,
        )
        .unwrap();
        assert_eq!(request.parts.len(), 2);
        assert_eq!(
            request.parts[0].content.segments[0],
            MessageSegment::text("你好。")
        );
        assert_eq!(
            request.parts[1].content.segments[0],
            MessageSegment::text("世界！")
        );
    }
}
