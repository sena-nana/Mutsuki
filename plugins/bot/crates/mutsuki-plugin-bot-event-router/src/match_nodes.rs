use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_FLOW_BOT_EVENT_TYPE, BotEvent, BotFlowTypeRef, BotNodeBinding, BotNodeCatalogFragment,
    BotNodeDescriptor, BotNodeInvocation, BotNodeOutput, BotNodePortDescriptor,
    BotNodePortDirection, BotNodeResult, BotNodeRole, BotTarget, MessageSegment,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, RunnerDescriptor, RunnerResult, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{RunnerDescriptorBuilder, map_work_batch_entries};
use serde::Deserialize;
use serde_json::json;

use crate::BOT_FLOW_ROUTER_PLUGIN_ID;

pub const BOT_FLOW_MATCH_RUNNER_ID: &str = "mutsuki.bot.router.flow.match";
pub const BOT_FLOW_EVENT_MATCH_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/event@1";
pub const BOT_FLOW_RATE_LIMIT_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/rate-limit@1";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EventMatchConfig {
    event_kinds: Vec<String>,
    account_ids: Vec<String>,
    actor_ids: Vec<String>,
    conversation_kinds: Vec<String>,
    roles: Vec<String>,
    must_mention_bot: bool,
    wake_words: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RateLimitConfig {
    scope: String,
    capacity: u32,
    refill_tokens: u32,
    refill_interval_ms: u64,
}

#[derive(Clone, Copy)]
struct Bucket {
    available_milli: u128,
    updated_at_ms: u64,
}

pub struct BotFlowMatchRunner {
    descriptor: RunnerDescriptor,
    buckets: BTreeMap<String, Bucket>,
}

impl Default for BotFlowMatchRunner {
    fn default() -> Self {
        Self {
            descriptor: match_descriptor(),
            buckets: BTreeMap::new(),
        }
    }
}

impl Runner for BotFlowMatchRunner {
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
                .map_err(|error| runtime_error(task, "decode", error))?;
            let event: BotEvent = serde_json::from_value(invocation.input.payload.value.clone())
                .map_err(|error| runtime_error(task, "event", error))?;
            let matched = match task.protocol_id.as_str() {
                BOT_FLOW_EVENT_MATCH_PROTOCOL_ID => {
                    let config: EventMatchConfig =
                        serde_json::from_value(invocation.config.clone())
                            .map_err(|error| runtime_error(task, "config", error))?;
                    event_matches(&event, &config)
                }
                BOT_FLOW_RATE_LIMIT_PROTOCOL_ID => {
                    let config: RateLimitConfig = serde_json::from_value(invocation.config.clone())
                        .map_err(|error| runtime_error(task, "config", error))?;
                    self.consume(&event, &config, event.time_ms.max(0).cast_unsigned())?
                }
                protocol => return Err(runtime_error(task, "protocol", protocol)),
            };
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.output = Some(
                serde_json::to_value(BotNodeResult {
                    outputs: vec![BotNodeOutput {
                        port_id: if matched { "matched" } else { "unmatched" }.into(),
                        event: invocation.input.clone(),
                    }],
                    metadata: Default::default(),
                })
                .map_err(|error| runtime_error(task, "output", error))?,
            );
            Ok(result)
        })
    }
}

impl BotFlowMatchRunner {
    fn consume(
        &mut self,
        event: &BotEvent,
        config: &RateLimitConfig,
        now_ms: u64,
    ) -> Result<bool, mutsuki_runtime_contracts::RuntimeError> {
        if config.capacity == 0 || config.refill_tokens == 0 || config.refill_interval_ms == 0 {
            return Err(mutsuki_runtime_contracts::RuntimeError::new(
                "bot.flow.rate_limit.invalid",
                BOT_FLOW_ROUTER_PLUGIN_ID,
                "rate limit values must be positive",
            ));
        }
        let subject = match config.scope.as_str() {
            "actor" => event
                .actor
                .as_ref()
                .map(|actor| format!("actor:{}", actor.user_id)),
            "conversation" => Some(format!("conversation:{}", conversation_key(&event.target))),
            "actor_and_conversation" => event.actor.as_ref().map(|actor| {
                format!(
                    "actor:{}|conversation:{}",
                    actor.user_id,
                    conversation_key(&event.target)
                )
            }),
            other => {
                return Err(mutsuki_runtime_contracts::RuntimeError::new(
                    "bot.flow.rate_limit.scope_invalid",
                    BOT_FLOW_ROUTER_PLUGIN_ID,
                    other,
                ));
            }
        };
        let Some(subject) = subject else {
            return Ok(false);
        };
        let capacity = u128::from(config.capacity) * 1_000;
        let bucket = self.buckets.entry(subject).or_insert(Bucket {
            available_milli: capacity,
            updated_at_ms: now_ms,
        });
        let elapsed = now_ms.saturating_sub(bucket.updated_at_ms);
        let refill = u128::from(elapsed)
            .saturating_mul(u128::from(config.refill_tokens))
            .saturating_mul(1_000)
            / u128::from(config.refill_interval_ms);
        bucket.available_milli = bucket.available_milli.saturating_add(refill).min(capacity);
        bucket.updated_at_ms = now_ms;
        if bucket.available_milli < 1_000 {
            return Ok(false);
        }
        bucket.available_milli -= 1_000;
        Ok(true)
    }
}

fn event_matches(event: &BotEvent, config: &EventMatchConfig) -> bool {
    let kind = serde_json::to_value(&event.kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "platform_specific".into());
    let conversation_kind = match &event.target {
        BotTarget::User { .. } => "private",
        BotTarget::Group { .. } => "group",
        BotTarget::GuildChannel { .. } => "channel",
        BotTarget::Conversation { .. } => "conversation",
        BotTarget::PlatformSpecific { .. } => "platform_specific",
    };
    let role = event
        .ext
        .get("bot.actor_role")
        .or_else(|| event.ext.get("qqbot.actor_role"))
        .and_then(serde_json::Value::as_str);
    let mentioned = event.message.as_ref().is_some_and(|message| {
        message.segments.iter().any(|segment| match segment {
            MessageSegment::MentionUser { user_id } => user_id == &event.bot.account_id,
            MessageSegment::MentionAll => true,
            _ => false,
        })
    });
    let wake = config.wake_words.is_empty()
        || event.message.as_ref().is_some_and(|message| {
            let text = message.plain_text().to_lowercase();
            config
                .wake_words
                .iter()
                .any(|word| !word.trim().is_empty() && text.contains(&word.to_lowercase()))
        });
    (config.event_kinds.is_empty() || config.event_kinds.iter().any(|value| value == &kind))
        && (config.account_ids.is_empty()
            || config
                .account_ids
                .iter()
                .any(|value| value == &event.bot.account_id))
        && (config.actor_ids.is_empty()
            || event
                .actor
                .as_ref()
                .is_some_and(|actor| config.actor_ids.iter().any(|value| value == &actor.user_id)))
        && (config.conversation_kinds.is_empty()
            || config
                .conversation_kinds
                .iter()
                .any(|value| value == conversation_kind))
        && (config.roles.is_empty()
            || role.is_some_and(|role| config.roles.iter().any(|value| value == role)))
        && (!config.must_mention_bot || mentioned)
        && wake
}

fn conversation_key(target: &BotTarget) -> String {
    match target {
        BotTarget::User { user_id } => format!("user:{user_id}"),
        BotTarget::Group { group_id } => format!("group:{group_id}"),
        BotTarget::GuildChannel {
            guild_id,
            channel_id,
        } => format!("channel:{guild_id}:{channel_id}"),
        BotTarget::Conversation { conversation_id } => format!("conversation:{conversation_id}"),
        BotTarget::PlatformSpecific { platform, kind, id } => {
            format!("platform:{platform}:{kind}:{id}")
        }
    }
}

pub fn match_node_catalog() -> BotNodeCatalogFragment {
    BotNodeCatalogFragment {
        nodes: vec![
            match_node(
                "mutsuki.bot.match.event",
                "事件匹配",
                BOT_FLOW_EVENT_MATCH_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "event_kinds": {"type": "array", "items": {"type": "string"}, "default": []},
                        "account_ids": {"type": "array", "items": {"type": "string"}, "default": []},
                        "actor_ids": {"type": "array", "items": {"type": "string"}, "default": []},
                        "conversation_kinds": {"type": "array", "items": {"type": "string"}, "default": []},
                        "roles": {"type": "array", "items": {"type": "string"}, "default": []},
                        "must_mention_bot": {"type": "boolean", "default": false},
                        "wake_words": {"type": "array", "items": {"type": "string"}, "default": []}
                    }
                }),
            ),
            match_node(
                "mutsuki.bot.match.rate_limit",
                "限流",
                BOT_FLOW_RATE_LIMIT_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["scope", "capacity", "refill_tokens", "refill_interval_ms"],
                    "properties": {
                        "scope": {"type": "string", "enum": ["actor", "conversation", "actor_and_conversation"]},
                        "capacity": {"type": "integer", "minimum": 1},
                        "refill_tokens": {"type": "integer", "minimum": 1},
                        "refill_interval_ms": {"type": "integer", "minimum": 1}
                    }
                }),
            ),
        ],
    }
}

fn match_node(
    node_type_id: &str,
    title: &str,
    protocol_id: &str,
    config_schema: serde_json::Value,
) -> BotNodeDescriptor {
    BotNodeDescriptor {
        node_type_id: node_type_id.into(),
        version: 1,
        title: title.into(),
        category: "匹配".into(),
        role: BotNodeRole::Match,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{protocol_id}"),
            protocol_id: protocol_id.into(),
            runner_hint: Some(BOT_FLOW_MATCH_RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "event".into(),
                title: "事件".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: true,
            },
            BotNodePortDescriptor {
                port_id: "matched".into(),
                title: "通过".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: false,
            },
            BotNodePortDescriptor {
                port_id: "unmatched".into(),
                title: "未通过".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: false,
            },
        ],
        config_schema,
    }
}

pub fn match_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_FLOW_MATCH_RUNNER_ID, BOT_FLOW_ROUTER_PLUGIN_ID)
        .accepted_protocol(BOT_FLOW_EVENT_MATCH_PROTOCOL_ID)
        .accepted_protocol(BOT_FLOW_RATE_LIMIT_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .build()
}

fn runtime_error(
    task: &Task,
    route: &str,
    error: impl std::fmt::Display,
) -> mutsuki_runtime_contracts::RuntimeError {
    let mut value = mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_FLOW_ROUTER_PLUGIN_ID,
        format!("{}.{}", task.task_id, route),
    );
    value.evidence.insert(
        "message".into(),
        mutsuki_runtime_contracts::ScalarValue::String(error.to_string()),
    );
    value
}
