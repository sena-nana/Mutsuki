//! Source nodes: which `BotEvent`s start a chain.
//!
//! The platform-neutral sources (`mutsuki.bot.message.created`, …) select on `BotEventKind` and
//! optionally narrow to one `platform` and/or `account`. The QQ sources
//! (`mutsuki.bot.qq.message.created`, …) are declared by the QQ adapter and persisted in user
//! graphs; they keep their ids and select the same kinds, restricted to `qqbot` events so a QQ
//! chain never receives another platform's conversation.

use mutsuki_bot_protocol::{
    BOT_FLOW_LIFECYCLE_EVENT_TYPE, BOT_FLOW_MEMBER_EVENT_TYPE, BOT_FLOW_MESSAGE_DELETED_EVENT_TYPE,
    BOT_FLOW_MESSAGE_EVENT_TYPE, BOT_FLOW_PLATFORM_EVENT_TYPE, BOT_FLOW_REACTION_EVENT_TYPE,
    BotEvent, BotEventKind, BotFlowNode, BotFlowTypeRef, BotNodeDescriptor, BotNodePortDescriptor,
    BotNodePortDirection, BotNodeRole, BotPlatform,
};
use serde::Deserialize;
use serde_json::json;

/// One source family: the neutral node id, its QQ counterpart, the received kind and port type.
struct SourceKind {
    neutral: &'static str,
    qq: &'static str,
    kind: &'static str,
    title: &'static str,
    output_type: &'static str,
}

const SOURCE_KINDS: &[SourceKind] = &[
    SourceKind {
        neutral: "mutsuki.bot.message.created",
        qq: "mutsuki.bot.qq.message.created",
        kind: "message_created",
        title: "收到消息",
        output_type: BOT_FLOW_MESSAGE_EVENT_TYPE,
    },
    SourceKind {
        neutral: "mutsuki.bot.message.updated",
        qq: "mutsuki.bot.qq.message.updated",
        kind: "message_updated",
        title: "消息更新",
        output_type: BOT_FLOW_MESSAGE_EVENT_TYPE,
    },
    SourceKind {
        neutral: "mutsuki.bot.message.deleted",
        qq: "mutsuki.bot.qq.message.deleted",
        kind: "message_deleted",
        title: "消息删除",
        output_type: BOT_FLOW_MESSAGE_DELETED_EVENT_TYPE,
    },
    SourceKind {
        neutral: "mutsuki.bot.reaction.added",
        qq: "mutsuki.bot.qq.reaction.added",
        kind: "reaction_added",
        title: "添加表情",
        output_type: BOT_FLOW_REACTION_EVENT_TYPE,
    },
    SourceKind {
        neutral: "mutsuki.bot.reaction.removed",
        qq: "mutsuki.bot.qq.reaction.removed",
        kind: "reaction_removed",
        title: "取消表情",
        output_type: BOT_FLOW_REACTION_EVENT_TYPE,
    },
    SourceKind {
        neutral: "mutsuki.bot.member.joined",
        qq: "mutsuki.bot.qq.member.joined",
        kind: "member_joined",
        title: "成员加入",
        output_type: BOT_FLOW_MEMBER_EVENT_TYPE,
    },
    SourceKind {
        neutral: "mutsuki.bot.member.left",
        qq: "mutsuki.bot.qq.member.left",
        kind: "member_left",
        title: "成员离开",
        output_type: BOT_FLOW_MEMBER_EVENT_TYPE,
    },
    SourceKind {
        neutral: "mutsuki.bot.account.connected",
        qq: "mutsuki.bot.qq.bot.connected",
        kind: "bot_connected",
        title: "机器人上线",
        output_type: BOT_FLOW_LIFECYCLE_EVENT_TYPE,
    },
    SourceKind {
        neutral: "mutsuki.bot.account.disconnected",
        qq: "mutsuki.bot.qq.bot.disconnected",
        kind: "bot_disconnected",
        title: "机器人下线",
        output_type: BOT_FLOW_LIFECYCLE_EVENT_TYPE,
    },
    SourceKind {
        neutral: "mutsuki.bot.platform.event",
        qq: "mutsuki.bot.qq.platform",
        kind: "platform_specific",
        title: "平台事件",
        output_type: BOT_FLOW_PLATFORM_EVENT_TYPE,
    },
];

/// Received kinds a Source node type selects; empty for plugin-typed sources, which are selected
/// by their ingress event-type selector alone.
pub fn source_kinds_for_node(node_type_id: &str) -> &'static [&'static str] {
    SOURCE_KINDS
        .iter()
        .find(|source| source.neutral == node_type_id || source.qq == node_type_id)
        .map_or(&[], |source| std::slice::from_ref(&source.kind))
}

pub fn event_matches_source_types(event: &BotEvent, types: &[&str]) -> bool {
    types.is_empty() || types.contains(&event_kind_id(event))
}

/// Every arm is a literal, so this borrows rather than allocating.
pub(crate) fn event_kind_id(event: &BotEvent) -> &'static str {
    match &event.kind {
        BotEventKind::MessageCreated => "message_created",
        BotEventKind::MessageUpdated => "message_updated",
        BotEventKind::MessageDeleted => "message_deleted",
        BotEventKind::MemberJoined => "member_joined",
        BotEventKind::MemberLeft => "member_left",
        BotEventKind::ReactionAdded => "reaction_added",
        BotEventKind::ReactionRemoved => "reaction_removed",
        BotEventKind::BotConnected => "bot_connected",
        BotEventKind::BotDisconnected => "bot_disconnected",
        BotEventKind::PlatformSpecific(_) => "platform_specific",
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct NeutralSourceConfig {
    platform: String,
    account: String,
}

/// Which events one Source node instance accepts, decoded once per graph revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceFilter {
    kinds: &'static [&'static str],
    platform: Option<String>,
    account: Option<String>,
    /// A neutral source whose stored config no longer decodes accepts nothing.
    rejects_all: bool,
}

impl SourceFilter {
    pub(crate) fn for_node(node: &BotFlowNode) -> Self {
        let kinds = source_kinds_for_node(&node.node_type_id);
        let Some(source) = SOURCE_KINDS
            .iter()
            .find(|source| source.neutral == node.node_type_id || source.qq == node.node_type_id)
        else {
            return Self {
                kinds,
                platform: None,
                account: None,
                rejects_all: false,
            };
        };
        if source.qq == node.node_type_id {
            return Self {
                kinds,
                platform: Some(BotPlatform::QqBot.as_str().to_owned()),
                account: None,
                rejects_all: false,
            };
        }
        let non_empty = |value: String| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        };
        match NeutralSourceConfig::deserialize(&node.config) {
            Ok(config) => Self {
                kinds,
                platform: non_empty(config.platform),
                account: non_empty(config.account),
                rejects_all: false,
            },
            Err(_) => Self {
                kinds,
                platform: None,
                account: None,
                rejects_all: true,
            },
        }
    }

    /// `event` is `None` when the ingress payload is a plugin-owned typed event.
    pub(crate) fn accepts(&self, event: Option<&BotEvent>) -> bool {
        if self.rejects_all {
            return false;
        }
        match event {
            Some(event) if event.is_self_sent_message() => false,
            Some(event) => {
                event_matches_source_types(event, self.kinds)
                    && self
                        .platform
                        .as_deref()
                        .is_none_or(|platform| event.platform.as_str() == platform)
                    && self
                        .account
                        .as_deref()
                        .is_none_or(|account| event.bot.account_id == account)
            }
            None => self.kinds.is_empty(),
        }
    }
}

/// Platform-neutral Source nodes, published in the Flow router catalog.
pub fn neutral_source_nodes() -> Vec<BotNodeDescriptor> {
    SOURCE_KINDS
        .iter()
        .map(|source| BotNodeDescriptor {
            node_type_id: source.neutral.into(),
            version: 1,
            title: source.title.into(),
            category: "事件".into(),
            role: BotNodeRole::Source,
            binding: None,
            ports: vec![BotNodePortDescriptor {
                port_id: "event".into(),
                title: source.title.into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(source.output_type, 1),
                required: false,
            }],
            config_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "platform": {
                        "type": "string",
                        "title": "平台",
                        "description": "只接收该平台的事件（如 qqbot、sandbox），留空接收所有平台"
                    },
                    "account": {
                        "type": "string",
                        "title": "机器人账号",
                        "description": "只接收该账号的事件，留空接收所有账号"
                    }
                }
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use mutsuki_bot_protocol::{
        BOT_SELF_SENT_EXT_KEY, BotAccountRef, BotExtMap, BotFlowNodePosition, BotTarget,
    };
    use serde_json::{Value, json};

    use super::*;

    fn node(node_type_id: &str, config: Value) -> BotFlowNode {
        BotFlowNode {
            node_id: "source".into(),
            node_type_id: node_type_id.into(),
            node_type_version: 1,
            config,
            source: None,
            position: BotFlowNodePosition::default(),
        }
    }

    fn event(platform: BotPlatform, account: &str, kind: BotEventKind) -> BotEvent {
        BotEvent {
            event_id: "e1".into(),
            platform: platform.clone(),
            bot: BotAccountRef {
                account_id: account.into(),
                platform,
            },
            kind,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: "g1".into(),
            },
            actor: None,
            message: None,
            raw: None,
            ext: BotExtMap::new(),
        }
    }

    #[test]
    fn neutral_source_selects_kind_and_optional_platform_and_account() {
        let qq = event(BotPlatform::QqBot, "qq-main", BotEventKind::MessageCreated);
        let sandbox = event(
            BotPlatform::Sandbox,
            "sandbox",
            BotEventKind::MessageCreated,
        );
        let joined = event(BotPlatform::Sandbox, "sandbox", BotEventKind::MemberJoined);

        let any = SourceFilter::for_node(&node("mutsuki.bot.message.created", json!({})));
        assert!(any.accepts(Some(&qq)));
        assert!(any.accepts(Some(&sandbox)));
        assert!(!any.accepts(Some(&joined)));
        assert!(
            !any.accepts(None),
            "a kind source never takes typed payloads"
        );

        let sandbox_only = SourceFilter::for_node(&node(
            "mutsuki.bot.message.created",
            json!({"platform": "sandbox"}),
        ));
        assert!(!sandbox_only.accepts(Some(&qq)));
        assert!(sandbox_only.accepts(Some(&sandbox)));

        let account = SourceFilter::for_node(&node(
            "mutsuki.bot.message.created",
            json!({"platform": "qqbot", "account": "other"}),
        ));
        assert!(!account.accepts(Some(&qq)));

        let broken = SourceFilter::for_node(&node(
            "mutsuki.bot.message.created",
            json!({"platfrom": "qqbot"}),
        ));
        assert!(!broken.accepts(Some(&qq)));
    }

    #[test]
    fn persisted_qq_sources_keep_their_kind_and_only_take_qq_events() {
        let filter = SourceFilter::for_node(&node("mutsuki.bot.qq.message.created", json!({})));
        assert!(filter.accepts(Some(&event(
            BotPlatform::QqBot,
            "qq-main",
            BotEventKind::MessageCreated
        ))));
        assert!(!filter.accepts(Some(&event(
            BotPlatform::Sandbox,
            "sandbox",
            BotEventKind::MessageCreated
        ))));
        assert_eq!(
            source_kinds_for_node("mutsuki.bot.qq.bot.connected"),
            source_kinds_for_node("mutsuki.bot.account.connected")
        );
    }

    #[test]
    fn self_sent_messages_never_start_a_chain() {
        let mut echo = event(
            BotPlatform::Sandbox,
            "sandbox",
            BotEventKind::MessageCreated,
        );
        echo.ext
            .insert(BOT_SELF_SENT_EXT_KEY.into(), Value::Bool(true));
        let filter = SourceFilter::for_node(&node("mutsuki.bot.message.created", json!({})));
        assert!(!filter.accepts(Some(&echo)));
    }

    #[test]
    fn plugin_typed_sources_take_only_typed_payloads() {
        let filter = SourceFilter::for_node(&node("mutsuki.bot.bilibili.notification", json!({})));
        assert!(filter.accepts(None));
    }

    #[test]
    fn neutral_catalog_mirrors_every_qq_source_kind() {
        let nodes = neutral_source_nodes();
        assert_eq!(nodes.len(), SOURCE_KINDS.len());
        assert!(nodes.iter().all(|node| node.role == BotNodeRole::Source
            && node.binding.is_none()
            && !source_kinds_for_node(&node.node_type_id).is_empty()));
    }
}
