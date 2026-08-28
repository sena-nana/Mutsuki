use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_EXT_LINK_URL, BOT_FLOW_BOT_EVENT_TYPE, BOT_FLOW_EMPTY_MENTION_PROTOCOL_ID,
    BOT_FLOW_LINK_PROTOCOL_ID, BOT_FLOW_PROBABILITY_PROTOCOL_ID, BotEvent, BotEventKind,
    BotFlowTypeRef, BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor, BotNodeInvocation,
    BotNodeOutput, BotNodePortDescriptor, BotNodePortDirection, BotNodeResult, BotNodeRole,
    BotTarget, MessageSegment,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, RunnerDescriptor, RunnerResult, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{RunnerDescriptorBuilder, map_work_batch_entries};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::BOT_FLOW_ROUTER_PLUGIN_ID;

pub const BOT_FLOW_MATCH_RUNNER_ID: &str = "mutsuki.bot.router.flow.match";
pub use mutsuki_bot_protocol::{
    BOT_FLOW_CONVERSATION_PROTOCOL_ID, BOT_FLOW_KEYWORD_PROTOCOL_ID, BOT_FLOW_MENTION_PROTOCOL_ID,
    BOT_FLOW_PREFIX_PROTOCOL_ID, BOT_FLOW_QQ_EVENT_PROTOCOL_ID, BOT_FLOW_RATE_LIMIT_PROTOCOL_ID,
    BOT_FLOW_ROLE_PROTOCOL_ID, BOT_FLOW_USER_PROTOCOL_ID,
};

pub const MATCH_PROTOCOL_IDS: &[&str] = &[
    BOT_FLOW_CONVERSATION_PROTOCOL_ID,
    BOT_FLOW_USER_PROTOCOL_ID,
    BOT_FLOW_ROLE_PROTOCOL_ID,
    BOT_FLOW_PREFIX_PROTOCOL_ID,
    BOT_FLOW_KEYWORD_PROTOCOL_ID,
    BOT_FLOW_LINK_PROTOCOL_ID,
    BOT_FLOW_MENTION_PROTOCOL_ID,
    BOT_FLOW_RATE_LIMIT_PROTOCOL_ID,
    BOT_FLOW_QQ_EVENT_PROTOCOL_ID,
    BOT_FLOW_EMPTY_MENTION_PROTOCOL_ID,
    BOT_FLOW_PROBABILITY_PROTOCOL_ID,
];

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConversationConfig {
    kinds: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct UserConfig {
    users: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RoleConfig {
    roles: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PrefixConfig {
    prefixes: Vec<String>,
    case_sensitive: bool,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct KeywordConfig {
    keywords: Vec<String>,
    mode: String,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LinkConfig {
    hosts: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RateLimitConfig {
    scope: String,
    max_count: u32,
    period_seconds: u32,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct QqEventConfig {
    event_types: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ProbabilityConfig {
    p: f64,
    #[serde(default)]
    users: Vec<String>,
    #[serde(default)]
    conversations: Vec<String>,
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
            let matched = match_event(self, &event, task.protocol_id.as_str(), &invocation.config)
                .map_err(|error| runtime_error(task, "config", error))?;
            let mut output_event = invocation.input.clone();
            if matched && task.protocol_id == BOT_FLOW_LINK_PROTOCOL_ID {
                attach_matched_urls(&mut output_event, &event, &invocation.config)
                    .map_err(|error| runtime_error(task, "link", error))?;
            }
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.output = Some(
                serde_json::to_value(BotNodeResult {
                    outputs: vec![BotNodeOutput {
                        port_id: if matched { "matched" } else { "unmatched" }.into(),
                        event: output_event,
                    }],
                    metadata: Default::default(),
                })
                .map_err(|error| runtime_error(task, "output", error))?,
            );
            Ok(result)
        })
    }
}

fn match_event(
    runner: &mut BotFlowMatchRunner,
    event: &BotEvent,
    protocol: &str,
    config: &Value,
) -> Result<bool, String> {
    Ok(match protocol {
        BOT_FLOW_CONVERSATION_PROTOCOL_ID => {
            let config: ConversationConfig = decode_config(config)?;
            !config.kinds.is_empty()
                && config
                    .kinds
                    .iter()
                    .any(|kind| kind == conversation_kind(&event.target))
        }
        BOT_FLOW_USER_PROTOCOL_ID => {
            let config: UserConfig = decode_config(config)?;
            let users = config
                .users
                .iter()
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            !users.is_empty()
                && event
                    .actor
                    .as_ref()
                    .is_some_and(|actor| users.iter().any(|user| *user == actor.user_id))
        }
        BOT_FLOW_ROLE_PROTOCOL_ID => {
            let config: RoleConfig = decode_config(config)?;
            let role = actor_role(event);
            !config.roles.is_empty()
                && role.is_some_and(|role| config.roles.iter().any(|value| value == role))
        }
        BOT_FLOW_PREFIX_PROTOCOL_ID => {
            let config: PrefixConfig = decode_config(config)?;
            prefix_matches(event, &config)
        }
        BOT_FLOW_KEYWORD_PROTOCOL_ID => {
            let config: KeywordConfig = decode_config(config)?;
            keyword_matches(event, &config)
        }
        BOT_FLOW_LINK_PROTOCOL_ID => {
            let config: LinkConfig = decode_config(config)?;
            !matching_link_urls(event, &config).is_empty()
        }
        BOT_FLOW_MENTION_PROTOCOL_ID => mentioned_bot(event),
        BOT_FLOW_RATE_LIMIT_PROTOCOL_ID => {
            let config: RateLimitConfig = decode_config(config)?;
            runner.consume(event, &config, event.time_ms.max(0).cast_unsigned())?
        }
        BOT_FLOW_QQ_EVENT_PROTOCOL_ID => {
            let config: QqEventConfig = decode_config(config)?;
            qq_event_type(event).is_some_and(|event_type| {
                !config.event_types.is_empty()
                    && config.event_types.iter().any(|item| item == &event_type)
            })
        }
        BOT_FLOW_EMPTY_MENTION_PROTOCOL_ID => empty_mention(event),
        BOT_FLOW_PROBABILITY_PROTOCOL_ID => {
            let config: ProbabilityConfig = decode_config(config)?;
            probability_matches(event, &config)
        }
        other => return Err(other.into()),
    })
}

fn decode_config<T: for<'de> Deserialize<'de>>(config: &Value) -> Result<T, String> {
    serde_json::from_value(config.clone()).map_err(|error| error.to_string())
}

impl BotFlowMatchRunner {
    fn consume(
        &mut self,
        event: &BotEvent,
        config: &RateLimitConfig,
        now_ms: u64,
    ) -> Result<bool, String> {
        if config.max_count == 0 || config.period_seconds == 0 {
            return Err("限流次数和周期必须大于 0".into());
        }
        let subject = match config.scope.as_str() {
            "user" | "actor" => event
                .actor
                .as_ref()
                .map(|actor| format!("user:{}", actor.user_id)),
            "conversation" => Some(format!("conversation:{}", event.target.conversation_key())),
            "user_and_conversation" | "actor_and_conversation" => {
                event.actor.as_ref().map(|actor| {
                    format!(
                        "user:{}|conversation:{}",
                        actor.user_id,
                        event.target.conversation_key()
                    )
                })
            }
            other => return Err(format!("未知限流对象 {other}")),
        };
        let Some(subject) = subject else {
            return Ok(false);
        };
        let capacity = u128::from(config.max_count) * 1_000;
        let period_ms = u128::from(config.period_seconds) * 1_000;
        let bucket = self.buckets.entry(subject).or_insert(Bucket {
            available_milli: capacity,
            updated_at_ms: now_ms,
        });
        let elapsed = u128::from(now_ms.saturating_sub(bucket.updated_at_ms));
        let refill = elapsed.saturating_mul(capacity) / period_ms;
        bucket.available_milli = bucket.available_milli.saturating_add(refill).min(capacity);
        bucket.updated_at_ms = now_ms;
        if bucket.available_milli < 1_000 {
            return Ok(false);
        }
        bucket.available_milli -= 1_000;
        Ok(true)
    }
}

fn event_kind_id(event: &BotEvent) -> String {
    match &event.kind {
        BotEventKind::MessageCreated => "message_created".into(),
        BotEventKind::MessageUpdated => "message_updated".into(),
        BotEventKind::MessageDeleted => "message_deleted".into(),
        BotEventKind::MemberJoined => "member_joined".into(),
        BotEventKind::MemberLeft => "member_left".into(),
        BotEventKind::ReactionAdded => "reaction_added".into(),
        BotEventKind::ReactionRemoved => "reaction_removed".into(),
        BotEventKind::BotConnected => "bot_connected".into(),
        BotEventKind::BotDisconnected => "bot_disconnected".into(),
        BotEventKind::PlatformSpecific(_) => "platform_specific".into(),
    }
}

fn conversation_kind(target: &BotTarget) -> &'static str {
    match target {
        BotTarget::User { .. } => "private",
        BotTarget::Group { .. } => "group",
        BotTarget::GuildChannel { .. } => "channel",
        BotTarget::Conversation { .. } => "conversation",
        BotTarget::PlatformSpecific { .. } => "platform_specific",
    }
}

fn qq_event_type(event: &BotEvent) -> Option<String> {
    event
        .ext
        .get("qqbot.event_type")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| match &event.kind {
            BotEventKind::PlatformSpecific(name) => Some(name.clone()),
            _ => None,
        })
}

fn actor_role(event: &BotEvent) -> Option<&str> {
    event
        .ext
        .get("bot.actor_role")
        .or_else(|| event.ext.get("qqbot.actor_role"))
        .and_then(Value::as_str)
}

fn mentioned_bot(event: &BotEvent) -> bool {
    if event
        .ext
        .get("qqbot.mentioned_bot")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    event.message.as_ref().is_some_and(|message| {
        message.segments.iter().any(|segment| {
            matches!(
                segment,
                MessageSegment::MentionUser { user_id } if user_id == &event.bot.account_id
            )
        })
    })
}

fn empty_mention(event: &BotEvent) -> bool {
    mentioned_bot(event)
        && message_text(event).is_some_and(|text| text.chars().all(char::is_whitespace))
}

fn probability_matches(event: &BotEvent, config: &ProbabilityConfig) -> bool {
    if !(0.0..=1.0).contains(&config.p) {
        return false;
    }
    if !config.users.is_empty() {
        let Some(actor) = event.actor.as_ref() else {
            return false;
        };
        if !config.users.iter().any(|user| user == &actor.user_id) {
            return false;
        }
    }
    if !config.conversations.is_empty()
        && !config
            .conversations
            .iter()
            .any(|value| value == &event.target.conversation_key())
    {
        return false;
    }
    if config.p >= 1.0 {
        return true;
    }
    if config.p <= 0.0 {
        return false;
    }
    let sample = deterministic_unit_sample(&event.event_id);
    sample < config.p
}

fn deterministic_unit_sample(event_id: &str) -> f64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in event_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    (hash % 10_000) as f64 / 10_000.0
}

fn message_text(event: &BotEvent) -> Option<String> {
    event.message.as_ref().map(|message| message.plain_text())
}

fn prefix_matches(event: &BotEvent, config: &PrefixConfig) -> bool {
    let Some(text) = message_text(event) else {
        return false;
    };
    let prefixes = config
        .prefixes
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if prefixes.is_empty() {
        return false;
    }
    if config.case_sensitive {
        prefixes.iter().any(|prefix| text.starts_with(*prefix))
    } else {
        let text = text.to_lowercase();
        prefixes
            .iter()
            .any(|prefix| text.starts_with(&prefix.to_lowercase()))
    }
}

fn keyword_matches(event: &BotEvent, config: &KeywordConfig) -> bool {
    let Some(text) = message_text(event) else {
        return false;
    };
    let keywords = config
        .keywords
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if keywords.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    let hits = keywords
        .iter()
        .filter(|word| text.contains(&word.to_lowercase()))
        .count();
    if config.mode == "all" {
        hits == keywords.len()
    } else {
        hits > 0
    }
}

fn matching_link_urls(event: &BotEvent, config: &LinkConfig) -> Vec<String> {
    let hosts = config
        .hosts
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if hosts.is_empty() {
        return Vec::new();
    }
    mutsuki_bot_link_parser::urls_from_event(event)
        .into_iter()
        .filter(|url| mutsuki_bot_link_parser::host_matches(url, &hosts))
        .map(|url| url.to_string())
        .collect()
}

fn attach_matched_urls(
    envelope: &mut mutsuki_bot_protocol::BotFlowEventEnvelope,
    event: &BotEvent,
    config: &Value,
) -> Result<(), String> {
    let config: LinkConfig = decode_config(config)?;
    let urls = matching_link_urls(event, &config);
    let Some(first) = urls.first() else {
        return Ok(());
    };
    let mut event = event.clone();
    event.ext.insert(BOT_EXT_LINK_URL.into(), json!(first));
    envelope.payload.value = serde_json::to_value(&event).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn source_kinds_for_node(node_type_id: &str) -> &'static [&'static str] {
    match node_type_id {
        "mutsuki.bot.qq.message.created" => &["message_created"],
        "mutsuki.bot.qq.message.updated" => &["message_updated"],
        "mutsuki.bot.qq.message.deleted" => &["message_deleted"],
        "mutsuki.bot.qq.reaction.added" => &["reaction_added"],
        "mutsuki.bot.qq.reaction.removed" => &["reaction_removed"],
        "mutsuki.bot.qq.member.joined" => &["member_joined"],
        "mutsuki.bot.qq.member.left" => &["member_left"],
        "mutsuki.bot.qq.bot.connected" => &["bot_connected"],
        "mutsuki.bot.qq.bot.disconnected" => &["bot_disconnected"],
        "mutsuki.bot.qq.platform" => &["platform_specific"],
        _ => &[],
    }
}

pub fn event_matches_source_types(event: &BotEvent, types: &[&str]) -> bool {
    types.is_empty() || types.iter().any(|kind| *kind == event_kind_id(event))
}

fn enum_items(values: &[(&str, &str)]) -> Value {
    json!({
        "type": "string",
        "enum": values.iter().map(|(value, _)| *value).collect::<Vec<_>>(),
        "enumTitles": values.iter().map(|(_, title)| *title).collect::<Vec<_>>(),
    })
}

pub fn match_node_catalog() -> BotNodeCatalogFragment {
    BotNodeCatalogFragment {
        nodes: vec![
            match_node(
                "mutsuki.bot.match.conversation",
                "会话类型",
                BOT_FLOW_CONVERSATION_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["kinds"],
                    "properties": {
                        "kinds": {
                            "type": "array",
                            "title": "会话",
                            "minItems": 1,
                            "uniqueItems": true,
                            "items": enum_items(&[
                                ("private", "私聊"),
                                ("group", "群"),
                                ("channel", "频道"),
                                ("conversation", "会话"),
                                ("platform_specific", "平台会话")
                            ])
                        }
                    }
                }),
            ),
            match_node(
                "mutsuki.bot.match.user",
                "用户匹配",
                BOT_FLOW_USER_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["users"],
                    "properties": {
                        "users": {
                            "type": "array",
                            "title": "用户",
                            "minItems": 1,
                            "items": {"type": "string", "minLength": 1, "title": "用户"}
                        }
                    }
                }),
            ),
            match_node(
                "mutsuki.bot.match.role",
                "角色匹配",
                BOT_FLOW_ROLE_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["roles"],
                    "properties": {
                        "roles": {
                            "type": "array",
                            "title": "角色",
                            "minItems": 1,
                            "uniqueItems": true,
                            "items": enum_items(&[
                                ("owner", "群主"),
                                ("administrator", "管理员"),
                                ("member", "成员")
                            ])
                        }
                    }
                }),
            ),
            match_node(
                "mutsuki.bot.match.prefix",
                "前缀匹配",
                BOT_FLOW_PREFIX_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["prefixes"],
                    "properties": {
                        "prefixes": {
                            "type": "array",
                            "title": "前缀",
                            "minItems": 1,
                            "items": {"type": "string", "minLength": 1}
                        },
                        "case_sensitive": {
                            "type": "boolean",
                            "title": "区分大小写",
                            "default": false
                        }
                    }
                }),
            ),
            match_node(
                "mutsuki.bot.match.keyword",
                "关键词匹配",
                BOT_FLOW_KEYWORD_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["keywords"],
                    "properties": {
                        "keywords": {
                            "type": "array",
                            "title": "关键词",
                            "minItems": 1,
                            "items": {"type": "string", "minLength": 1}
                        },
                        "mode": {
                            "type": "string",
                            "title": "匹配方式",
                            "enum": ["any", "all"],
                            "enumTitles": ["包含任一", "包含全部"],
                            "default": "any"
                        }
                    }
                }),
            ),
            match_node(
                "mutsuki.bot.match.link",
                "链接匹配",
                BOT_FLOW_LINK_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["hosts"],
                    "properties": {
                        "hosts": {
                            "type": "array",
                            "title": "域名",
                            "minItems": 1,
                            "items": {"type": "string", "minLength": 1, "title": "域名"}
                        }
                    }
                }),
            ),
            match_node(
                "mutsuki.bot.match.mention",
                "提及机器人",
                BOT_FLOW_MENTION_PROTOCOL_ID,
                json!({"type": "object", "additionalProperties": false}),
            ),
            match_node(
                "mutsuki.bot.match.rate_limit",
                "限流",
                BOT_FLOW_RATE_LIMIT_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["scope", "max_count", "period_seconds"],
                    "properties": {
                        "scope": {
                            "type": "string",
                            "title": "限制对象",
                            "enum": ["user", "conversation", "user_and_conversation"],
                            "enumTitles": ["每个用户", "每个会话", "同一用户同一会话"]
                        },
                        "max_count": {
                            "type": "integer",
                            "title": "最多次数",
                            "minimum": 1,
                            "default": 1
                        },
                        "period_seconds": {
                            "type": "integer",
                            "title": "统计周期（秒）",
                            "minimum": 1,
                            "default": 60
                        }
                    }
                }),
            ),
            match_node(
                "mutsuki.bot.match.qq_event",
                "QQ 事件",
                BOT_FLOW_QQ_EVENT_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["event_types"],
                    "properties": {
                        "event_types": {
                            "type": "array",
                            "title": "QQ 事件类型",
                            "minItems": 1,
                            "items": {"type": "string", "minLength": 1, "title": "事件类型"}
                        }
                    }
                }),
            ),
            match_node(
                "mutsuki.bot.match.empty_mention",
                "空提及",
                BOT_FLOW_EMPTY_MENTION_PROTOCOL_ID,
                json!({"type": "object", "additionalProperties": false}),
            ),
            match_node(
                "mutsuki.bot.match.probability",
                "概率匹配",
                BOT_FLOW_PROBABILITY_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["p"],
                    "properties": {
                        "p": {
                            "type": "number",
                            "title": "触发概率",
                            "minimum": 0,
                            "maximum": 1,
                            "default": 0.1
                        },
                        "users": {
                            "type": "array",
                            "title": "用户白名单",
                            "items": {"type": "string", "minLength": 1}
                        },
                        "conversations": {
                            "type": "array",
                            "title": "会话白名单",
                            "items": {"type": "string", "minLength": 1}
                        }
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
    config_schema: Value,
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
                required: false,
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
    let mut builder =
        RunnerDescriptorBuilder::new(BOT_FLOW_MATCH_RUNNER_ID, BOT_FLOW_ROUTER_PLUGIN_ID);
    for protocol in MATCH_PROTOCOL_IDS {
        builder = builder.accepted_protocol(*protocol);
    }
    builder
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

#[cfg(test)]
mod tests {
    use mutsuki_bot_protocol::{
        BOT_FLOW_MESSAGE_EVENT_TYPE, BotAccountRef, BotMessage, BotPlatform, BotUser,
    };

    use super::*;

    fn event(text: &str) -> BotEvent {
        BotEvent {
            event_id: "e1".into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "bot".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: "g1".into(),
            },
            actor: Some(BotUser {
                user_id: "u1".into(),
                display_name: Some("Alice".into()),
                avatar_url: None,
            }),
            message: Some(BotMessage::text(
                BotTarget::Group {
                    group_id: "g1".into(),
                },
                text,
            )),
            raw: None,
            ext: Default::default(),
        }
    }

    #[test]
    fn mention_match_accepts_platform_flag() {
        let mut flagged = event("hello");
        flagged
            .ext
            .insert("qqbot.mentioned_bot".into(), json!(true));
        assert!(mentioned_bot(&flagged));
        assert!(!mentioned_bot(&event("hello")));
    }

    fn event_with_segments(segments: Vec<MessageSegment>) -> BotEvent {
        let mut event = event("");
        if let Some(message) = event.message.as_mut() {
            message.segments = segments;
        }
        event
    }

    #[test]
    fn mention_all_is_not_mentioned_bot() {
        let only_all = event_with_segments(vec![MessageSegment::MentionAll]);
        assert!(!mentioned_bot(&only_all));
        assert!(!empty_mention(&only_all));
    }

    #[test]
    fn mention_user_matching_bot_is_mentioned_bot() {
        let self_mention = event_with_segments(vec![MessageSegment::MentionUser {
            user_id: "bot".into(),
        }]);
        assert!(mentioned_bot(&self_mention));
        assert!(empty_mention(&self_mention));
        let other = event_with_segments(vec![MessageSegment::MentionUser {
            user_id: "other".into(),
        }]);
        assert!(!mentioned_bot(&other));
        assert!(!empty_mention(&other));
    }

    #[test]
    fn prefix_and_keyword_nodes_use_message_text() {
        let event = event("/echo hello");
        assert!(prefix_matches(
            &event,
            &PrefixConfig {
                prefixes: vec!["/".into(), "!".into()],
                case_sensitive: false,
            },
        ));
        assert!(!prefix_matches(
            &event,
            &PrefixConfig {
                prefixes: vec!["!".into()],
                case_sensitive: false,
            },
        ));
        assert!(keyword_matches(
            &event,
            &KeywordConfig {
                keywords: vec!["echo".into(), "ping".into()],
                mode: "any".into(),
            },
        ));
        assert!(!keyword_matches(
            &event,
            &KeywordConfig {
                keywords: vec!["echo".into(), "ping".into()],
                mode: "all".into(),
            },
        ));
    }

    #[test]
    fn link_node_matches_card_json_and_plain_urls() {
        let card = event_with_segments(vec![MessageSegment::platform_specific(
            "qqbot",
            "ark_data",
            json!({"meta": {"jumpUrl": "https://b23.tv/abc"}}),
        )]);
        let bili = LinkConfig {
            hosts: vec!["b23.tv".into(), "bilibili.com".into()],
        };
        let mihuashi = LinkConfig {
            hosts: vec!["mihuashi.com".into()],
        };
        assert_eq!(
            matching_link_urls(&card, &bili),
            vec!["https://b23.tv/abc".to_owned()]
        );
        assert!(matching_link_urls(&card, &mihuashi).is_empty());
        let text = event("https://www.mihuashi.com/profiles/1");
        assert_eq!(
            matching_link_urls(&text, &mihuashi),
            vec!["https://www.mihuashi.com/profiles/1".to_owned()]
        );
    }

    #[test]
    fn catalog_is_a_business_match_series() {
        let catalog = match_node_catalog();
        // The node type ids are what a stored flow references, so the series is pinned by id.
        // Titles are console copy and are free to change without breaking a saved flow.
        let node_type_ids = catalog
            .nodes
            .iter()
            .map(|node| node.node_type_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            node_type_ids,
            [
                "mutsuki.bot.match.conversation",
                "mutsuki.bot.match.user",
                "mutsuki.bot.match.role",
                "mutsuki.bot.match.prefix",
                "mutsuki.bot.match.keyword",
                "mutsuki.bot.match.link",
                "mutsuki.bot.match.mention",
                "mutsuki.bot.match.rate_limit",
                "mutsuki.bot.match.qq_event",
                "mutsuki.bot.match.empty_mention",
                "mutsuki.bot.match.probability",
            ]
        );
        let user = catalog
            .nodes
            .iter()
            .find(|node| node.node_type_id == "mutsuki.bot.match.user")
            .unwrap();
        // The config a flow stores is keyed by these field names; the low-level identifiers the
        // node used to expose must stay out of the operator-facing surface.
        let properties = user.config_schema["properties"].as_object().unwrap();
        assert_eq!(properties.keys().collect::<Vec<_>>(), ["users"]);
        assert!(!properties.contains_key("account_id"));
        assert!(!properties.contains_key("actor_ids"));
        let prefix = catalog
            .nodes
            .iter()
            .find(|node| node.node_type_id == "mutsuki.bot.match.prefix")
            .unwrap();
        assert_eq!(prefix.ports[0].event_type.type_id, BOT_FLOW_BOT_EVENT_TYPE);
        assert!(
            BotFlowTypeRef::new(BOT_FLOW_MESSAGE_EVENT_TYPE, 1)
                .assigns_to(&BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1))
        );
    }

    #[test]
    fn source_event_types_filter_received_kinds() {
        let event = event("hi");
        assert!(event_matches_source_types(&event, &[]));
        assert!(event_matches_source_types(&event, &["message_created"]));
        assert!(!event_matches_source_types(&event, &["member_joined"]));
        assert_eq!(
            source_kinds_for_node("mutsuki.bot.qq.message.created"),
            &["message_created"]
        );
    }

    #[test]
    fn empty_mention_requires_mention_without_text() {
        let mut only_mention = event("");
        only_mention
            .ext
            .insert("qqbot.mentioned_bot".into(), json!(true));
        assert!(empty_mention(&only_mention));
        let mut with_text = event("hello");
        with_text
            .ext
            .insert("qqbot.mentioned_bot".into(), json!(true));
        assert!(!empty_mention(&with_text));
    }

    #[test]
    fn probability_node_is_deterministic_for_event_id() {
        let event = event("hello");
        let always = ProbabilityConfig {
            p: 1.0,
            users: Vec::new(),
            conversations: Vec::new(),
        };
        let never = ProbabilityConfig {
            p: 0.0,
            users: Vec::new(),
            conversations: Vec::new(),
        };
        assert!(probability_matches(&event, &always));
        assert!(!probability_matches(&event, &never));
        let sample = ProbabilityConfig {
            p: 0.5,
            users: Vec::new(),
            conversations: Vec::new(),
        };
        let first = probability_matches(&event, &sample);
        let second = probability_matches(&event, &sample);
        assert_eq!(first, second);
    }
}
