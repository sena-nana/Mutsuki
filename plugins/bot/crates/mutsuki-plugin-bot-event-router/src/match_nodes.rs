use std::collections::BTreeMap;

use mutsuki_bot_protocol::{
    BOT_FLOW_BOT_EVENT_TYPE, BotEvent, BotEventKind, BotFlowTypeRef, BotNodeBinding,
    BotNodeCatalogFragment, BotNodeDescriptor, BotNodeInvocation, BotNodeOutput,
    BotNodePortDescriptor, BotNodePortDirection, BotNodeResult, BotNodeRole, BotTarget,
    MessageSegment,
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
pub const BOT_FLOW_CONVERSATION_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/conversation@1";
pub const BOT_FLOW_USER_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/user@1";
pub const BOT_FLOW_ROLE_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/role@1";
pub const BOT_FLOW_PREFIX_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/prefix@1";
pub const BOT_FLOW_KEYWORD_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/keyword@1";
pub const BOT_FLOW_MENTION_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/mention@1";
pub const BOT_FLOW_RATE_LIMIT_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/rate-limit@1";
pub const BOT_FLOW_QQ_EVENT_PROTOCOL_ID: &str = "mutsuki.bot.flow.match/qq-event@1";

pub const MATCH_PROTOCOL_IDS: &[&str] = &[
    BOT_FLOW_CONVERSATION_PROTOCOL_ID,
    BOT_FLOW_USER_PROTOCOL_ID,
    BOT_FLOW_ROLE_PROTOCOL_ID,
    BOT_FLOW_PREFIX_PROTOCOL_ID,
    BOT_FLOW_KEYWORD_PROTOCOL_ID,
    BOT_FLOW_MENTION_PROTOCOL_ID,
    BOT_FLOW_RATE_LIMIT_PROTOCOL_ID,
    BOT_FLOW_QQ_EVENT_PROTOCOL_ID,
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
            "conversation" => Some(format!("conversation:{}", conversation_key(&event.target))),
            "user_and_conversation" | "actor_and_conversation" => {
                event.actor.as_ref().map(|actor| {
                    format!(
                        "user:{}|conversation:{}",
                        actor.user_id,
                        conversation_key(&event.target)
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
        message.segments.iter().any(|segment| match segment {
            MessageSegment::MentionUser { user_id } => user_id == &event.bot.account_id,
            MessageSegment::MentionAll => true,
            _ => false,
        })
    })
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
    fn catalog_is_a_business_match_series() {
        let catalog = match_node_catalog();
        let titles = catalog
            .nodes
            .iter()
            .map(|node| node.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            [
                "会话类型",
                "用户匹配",
                "角色匹配",
                "前缀匹配",
                "关键词匹配",
                "提及机器人",
                "限流",
                "QQ 事件",
            ]
        );
        assert!(
            !catalog
                .nodes
                .iter()
                .any(|node| node.node_type_id == "mutsuki.bot.match.event")
        );
        let user = catalog
            .nodes
            .iter()
            .find(|node| node.node_type_id == "mutsuki.bot.match.user")
            .unwrap();
        let schema = user.config_schema.to_string();
        assert!(schema.contains("用户"));
        assert!(!schema.contains("account_id"));
        assert!(!schema.contains("actor_ids"));
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
}
