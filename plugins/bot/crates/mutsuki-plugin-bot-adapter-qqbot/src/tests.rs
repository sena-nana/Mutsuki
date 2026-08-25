use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use mutsuki_bot_protocol::{
    BOT_FLOW_BOT_EVENT_TYPE, BOT_FLOW_DELIVERY_REPLY_TYPE, BOT_MEDIA_UPLOAD_PROTOCOL_ID,
    BOT_MESSAGE_RECALL_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID,
    BOT_QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID, BotConversationKind, BotDeliveryContent, BotEvent,
    BotEventKind, BotFlowContext, BotFlowEventEnvelope, BotFlowPayload, BotFlowTypeRef,
    BotMediaKind, BotMessage, BotMessageRecallRequest, BotNodeInvocation, BotNodeResult,
    BotReplyDeliveryPart, BotReplyDeliveryRequest, BotTarget, BotUser, DeliveryPolicy,
    MessageSegment, QQ_CONVERSATION_REF_VERSION, QQBOT_ACCOUNT_GET_PROTOCOL_ID,
    QQBOT_GATEWAY_STATUS_PROTOCOL_ID, QQBOT_OPENAPI_PERMANENT_ERROR,
    QQBOT_OPENAPI_RATE_LIMITED_ERROR, QQBOT_RAW_CALL_PROTOCOL_ID, QqConversationRef,
    QqMessageSegmentKind, QqPermissionRequirement,
};
use mutsuki_runtime_contracts::{
    BatchEntry, BatchKey, BatchPayload, CompletionBatch, DispatchLane, OrderingRequirement,
    RunnerResult, RunnerSideEffect, RuntimeError, Task, WorkBatch, WorkResourcePlan,
};
use mutsuki_runtime_core::{Runner, RunnerContext};
use serde_json::{Value, json};

use crate::adapter::qq_self_user;
use crate::api::{
    HttpMethod, MediaChunk, QqAuthManager, QqMediaError, QqMediaProvider, QqOpenApiError,
    QqOpenApiTransport,
};
use crate::config::{DEFAULT_QQBOT_INTENTS, QqBotConfig};
use crate::gateway::{GatewayAction, QqGatewayPump};
use crate::tasks::{
    QQBOT_GATEWAY_FRAME_PROTOCOL_ID, QQBOT_OPENAPI_RUNNER_ID, QqGatewayMapRunner, QqOpenApiRunner,
    apply_forward_fold, openapi_descriptor, qqbot_adapter_manifest,
};
use crate::{
    QqBotClients, QqHttpClient, QqHttpRequest, QqHttpResponse, QqIdSource, StaticQqCredentials,
};

fn decode_ingress_event(task: &Task) -> BotEvent {
    let envelope = task
        .payload
        .decode_shared::<BotFlowEventEnvelope>()
        .unwrap();
    assert_eq!(envelope.payload.event_type.type_id, BOT_FLOW_BOT_EVENT_TYPE);
    serde_json::from_value(envelope.payload.value.clone()).unwrap()
}

#[test]
fn gateway_pump_creates_internal_frame_tasks_and_deduplicates() {
    let mut pump = QqGatewayPump::new();
    let frame = json!({
        "op": 0,
        "s": 23,
        "t": "GROUP_MESSAGE_CREATE",
        "id": "GROUP_MESSAGE_CREATE:event",
        "d": {"id": "message-id", "content": "hi"}
    });

    let task = pump.handle_raw_frame(frame.clone(), 9).unwrap().unwrap();

    assert_eq!(task.protocol_id, QQBOT_GATEWAY_FRAME_PROTOCOL_ID);
    assert_eq!(task.registry_generation, 9);
    assert!(matches!(
        pump.pop_action(),
        Some(GatewayAction::DispatchTask(_))
    ));
    assert!(pump.handle_raw_frame(frame, 9).unwrap().is_none());
}

#[test]
fn gateway_runner_maps_qqbot_message_to_standard_bot_event() {
    let mut runner = QqGatewayMapRunner::new(1, "main");
    let mut task = Task::new(
        "gateway-task",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 24,
            "t": "C2C_MESSAGE_CREATE",
            "id": "C2C_MESSAGE_CREATE:event",
            "d": {
                "id": "message-id",
                "content": "ping",
                "author": {"user_openid": "USER_OPENID"}
            }
        }),
    );
    task.registry_generation = 1;

    let result = run_one(&mut runner, task).unwrap();

    assert_eq!(result.tasks.len(), 1);
    let event = decode_ingress_event(&result.tasks[0]);
    assert_eq!(event.kind, BotEventKind::MessageCreated);
    let message = event.message.unwrap();
    assert_eq!(message.plain_text(), "ping");
    assert_eq!(message.time_ms, None);
}

#[test]
fn gateway_runner_maps_channel_mentions_and_quote_context() {
    let mut runner = QqGatewayMapRunner::new(1, "main");
    let raw = json!({
        "op": 0,
        "s": 25,
        "t": "AT_MESSAGE_CREATE",
        "id": "channel-event",
        "d": {
            "id": "channel-message",
            "guild_id": "guild",
            "channel_id": "channel",
            "content": "hello <@bot>",
            "thread_id": "thread",
            "mentions": [{"id": "bot", "is_you": true}],
            "message_reference": {"message_id": "quoted"},
            "author": {"id": "actor"}
        }
    });
    let mut pump = QqGatewayPump::with_account("main", 8);
    let task = pump.handle_raw_frame(raw, 1).unwrap().unwrap();

    let result = run_one(&mut runner, task).unwrap();
    let event = decode_ingress_event(&result.tasks[0]);
    assert_eq!(
        event.target,
        BotTarget::GuildChannel {
            guild_id: "guild".into(),
            channel_id: "channel".into()
        }
    );
    assert_eq!(
        event.message.as_ref().unwrap().reply_to.as_deref(),
        Some("quoted")
    );
    assert_eq!(event.ext["qqbot.mentioned_bot"], Value::Bool(true));
    assert_eq!(event.ext["qqbot.thread_id"], Value::String("thread".into()));
    assert_eq!(event.ext["qqbot.sequence"], Value::from(25));
    assert!(
        event
            .message
            .unwrap()
            .segments
            .iter()
            .any(|segment| matches!(
                segment,
                MessageSegment::MentionUser { user_id } if user_id == "bot"
            ))
    );
}

#[test]
fn gateway_pump_and_runner_map_available_message_update_and_delete_events() {
    let mut pump = QqGatewayPump::with_account("main", 16);
    let mut runner = QqGatewayMapRunner::new(1, "main");
    for (sequence, event_type, expected_kind) in [
        (30, "MESSAGE_UPDATE", BotEventKind::MessageUpdated),
        (31, "MESSAGE_DELETE", BotEventKind::MessageDeleted),
        (32, "PUBLIC_MESSAGE_DELETE", BotEventKind::MessageDeleted),
        (33, "DIRECT_MESSAGE_DELETE", BotEventKind::MessageDeleted),
    ] {
        let task = pump
            .handle_raw_frame(
                json!({
                    "op": 0,
                    "s": sequence,
                    "t": event_type,
                    "id": format!("event-{sequence}"),
                    "d": {
                        "id": format!("message-{sequence}"),
                        "guild_id": "guild",
                        "channel_id": "channel",
                        "author": {"id": "actor"}
                    }
                }),
                1,
            )
            .unwrap()
            .unwrap();
        let result = run_one(&mut runner, task).unwrap();
        let event = decode_ingress_event(&result.tasks[0]);
        assert_eq!(event.kind, expected_kind);
        assert_eq!(event.ext["qqbot.sequence"], Value::from(sequence));
    }
}

#[test]
fn capability_matrix_follows_intents_and_resource_provider_configuration() {
    let mut config = QqBotConfig::new("main", "app");
    config.gateway_intents = 1 << 25;
    config.max_retry_attempts = 4;
    config.retry_base_delay_ms = 125;
    config.retry_max_delay_ms = 2_000;
    config.gateway_rate_limit_delay_ms = 30_000;
    let text_only = config.capability_matrix();
    assert_eq!(
        text_only.conversation_kinds,
        vec![BotConversationKind::Private, BotConversationKind::Group]
    );
    assert!(text_only.inbound_media.is_empty());
    assert!(text_only.outbound_media.is_empty());
    assert_eq!(text_only.configured_intents, 1 << 25);
    assert_eq!(text_only.shard, [0, 1]);
    assert!(
        text_only
            .inbound_segments
            .contains(&QqMessageSegmentKind::Reply)
    );
    assert!(
        text_only
            .outbound_segments
            .contains(&QqMessageSegmentKind::MentionUser)
    );
    assert!(
        text_only
            .outbound_segments
            .contains(&QqMessageSegmentKind::Reply)
    );
    assert!(
        text_only
            .outbound_segments
            .contains(&QqMessageSegmentKind::Quote)
    );
    assert!(
        text_only
            .outbound_segments
            .contains(&QqMessageSegmentKind::Markdown)
    );
    assert!(
        text_only
            .outbound_segments
            .contains(&QqMessageSegmentKind::Keyboard)
    );
    assert!(
        text_only
            .inbound_segments
            .contains(&QqMessageSegmentKind::Markdown)
    );
    assert!(text_only.quote);
    assert!(text_only.rate_limit.server_driven);
    assert!(text_only.rate_limit.honors_retry_after);
    assert_eq!(text_only.rate_limit.max_retry_attempts, 4);
    assert_eq!(text_only.rate_limit.retry_base_delay_ms, 125);
    assert_eq!(text_only.rate_limit.retry_max_delay_ms, 2_000);
    assert_eq!(text_only.rate_limit.gateway_rate_limit_delay_ms, 30_000);
    assert!(
        text_only
            .required_permissions
            .contains(&QqPermissionRequirement::ReadC2cMessages)
    );

    config.gateway_intents |= 1 << 30;
    config.media_provider_id = Some("memory".into());
    let media = config.capability_matrix();
    assert!(
        media
            .conversation_kinds
            .contains(&BotConversationKind::Channel)
    );
    assert!(
        !media
            .outbound_conversation_kinds
            .contains(&BotConversationKind::Channel)
    );
    assert!(media.inbound_media.contains(&BotMediaKind::Image));
    assert!(
        media
            .inbound_segments
            .contains(&QqMessageSegmentKind::Audio)
    );
    assert!(
        media
            .outbound_segments
            .contains(&QqMessageSegmentKind::File)
    );
    assert!(
        media
            .required_permissions
            .contains(&QqPermissionRequirement::ReadGuildAtMessages)
    );
    assert_eq!(
        media.upload.max_bytes_by_kind[&BotMediaKind::File],
        100 * 1024 * 1024
    );
}

#[test]
fn gateway_runner_uses_official_group_member_openid_and_c2c_id_fallbacks() {
    let mut runner = QqGatewayMapRunner::new(1, "main");
    let group = Task::new(
        "group",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 1,
            "t": "GROUP_MESSAGE_CREATE",
            "id": "group-event",
            "d": {
                "id": "group-message",
                "group_openid": "GROUP_OPENID",
                "content": "hello",
                "timestamp": "2026-07-11T10:00:00+08:00",
                "author": {"member_openid": "MEMBER_OPENID", "username": "member"}
            }
        }),
    );
    let c2c = Task::new(
        "c2c",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 2,
            "t": "C2C_MESSAGE_CREATE",
            "id": "c2c-event",
            "d": {
                "id": "c2c-message",
                "content": "hello",
                "author": {"id": "USER_OPENID", "username": "user"}
            }
        }),
    );

    let completion = run_tasks(&mut runner, vec![group, c2c]);
    let events = completion
        .results
        .iter()
        .map(|entry| decode_ingress_event(&entry.result.as_ref().unwrap().tasks[0]))
        .collect::<Vec<_>>();

    assert_eq!(events[0].actor.as_ref().unwrap().user_id, "MEMBER_OPENID");
    assert_eq!(events[0].time_ms, 1_783_735_200_000);
    assert_eq!(
        events[0].message.as_ref().unwrap().time_ms,
        Some(1_783_735_200_000)
    );
    assert_eq!(events[1].actor.as_ref().unwrap().user_id, "USER_OPENID");
    assert_eq!(
        events[1].target,
        BotTarget::User {
            user_id: "USER_OPENID".into()
        }
    );
}

#[test]
fn gateway_runner_copies_group_name_into_event_ext() {
    let mut runner = QqGatewayMapRunner::new(1, "main");
    let group = Task::new(
        "group",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 1,
            "t": "GROUP_AT_MESSAGE_CREATE",
            "id": "group-event",
            "d": {
                "id": "group-message",
                "group_openid": "GROUP_OPENID",
                "group_name": "读书分享会",
                "content": "hello",
                "author": {"member_openid": "MEMBER_OPENID"}
            }
        }),
    );

    let completion = run_tasks(&mut runner, vec![group]);
    let event = decode_ingress_event(&completion.results[0].result.as_ref().unwrap().tasks[0]);

    assert_eq!(
        event.ext.get("qqbot.group_name").and_then(Value::as_str),
        Some("读书分享会")
    );
}

#[test]
fn gateway_runner_synthesizes_group_qqapp_avatar_and_keeps_channel_avatar() {
    let mut runner = QqGatewayMapRunner::with_app_id(1, "main", "APP_ID");
    let group = Task::new(
        "group",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 1,
            "t": "GROUP_AT_MESSAGE_CREATE",
            "id": "group-event",
            "d": {
                "id": "group-message",
                "group_openid": "GROUP_OPENID",
                "content": "hello",
                "author": {"member_openid": "MEMBER_OPENID", "username": "member"}
            }
        }),
    );
    let channel = Task::new(
        "channel",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 2,
            "t": "AT_MESSAGE_CREATE",
            "id": "channel-event",
            "d": {
                "id": "channel-message",
                "guild_id": "guild",
                "channel_id": "channel",
                "content": "hello",
                "author": {
                    "id": "GUILD_USER",
                    "username": "guild-user",
                    "avatar": "https://example.test/guild-avatar.png"
                }
            }
        }),
    );

    let completion = run_tasks(&mut runner, vec![group, channel]);
    let events = completion
        .results
        .iter()
        .map(|entry| decode_ingress_event(&entry.result.as_ref().unwrap().tasks[0]))
        .collect::<Vec<_>>();

    assert_eq!(
        events[0].actor.as_ref().unwrap().avatar_url.as_deref(),
        Some("https://q.qlogo.cn/qqapp/APP_ID/MEMBER_OPENID/640")
    );
    assert_eq!(
        events[1].actor.as_ref().unwrap().avatar_url.as_deref(),
        Some("https://example.test/guild-avatar.png")
    );
}

#[test]
fn qq_self_user_keeps_avatar_url_and_synthesizes_qqapp_fallback() {
    let named = qq_self_user(
        &json!({
            "id": "BOT_OPENID",
            "username": "mutsuki",
            "avatar": "https://example.test/bot.png"
        }),
        "APP_ID",
    )
    .unwrap();
    assert_eq!(
        named,
        BotUser {
            user_id: "BOT_OPENID".into(),
            display_name: Some("mutsuki".into()),
            avatar_url: Some("https://example.test/bot.png".into()),
        }
    );

    let synthesized = qq_self_user(&json!({"id": "BOT_OPENID", "nick": "bot"}), "APP_ID").unwrap();
    assert_eq!(synthesized.display_name.as_deref(), Some("bot"));
    assert_eq!(
        synthesized.avatar_url.as_deref(),
        Some("https://q.qlogo.cn/qqapp/APP_ID/BOT_OPENID/640")
    );

    let http_cdn = qq_self_user(
        &json!({
            "id": "BOT_OPENID",
            "avatar": "http://thirdqq.qlogo.cn/g?b=oidb&k=TEST&s=0"
        }),
        "APP_ID",
    )
    .unwrap();
    assert_eq!(
        http_cdn.avatar_url.as_deref(),
        Some("https://thirdqq.qlogo.cn/g?b=oidb&k=TEST&s=0")
    );
}

#[test]
fn gateway_runner_maps_ready_user_as_bot_self() {
    let mut runner = QqGatewayMapRunner::with_app_id(1, "main", "APP_ID");
    let ready = Task::new(
        "ready",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 1,
            "t": "READY",
            "id": "ready-event",
            "d": {
                "session_id": "SESSION",
                "user": {"id": "BOT_OPENID", "username": "mutsuki"}
            }
        }),
    );

    let completion = run_tasks(&mut runner, vec![ready]);
    let event = decode_ingress_event(&completion.results[0].result.as_ref().unwrap().tasks[0]);
    assert_eq!(event.kind, BotEventKind::BotConnected);
    assert_eq!(
        event.actor,
        Some(BotUser {
            user_id: "BOT_OPENID".into(),
            display_name: Some("mutsuki".into()),
            avatar_url: Some("https://q.qlogo.cn/qqapp/APP_ID/BOT_OPENID/640".into()),
        })
    );
}

#[test]
fn gateway_runner_maps_lifecycle_seconds_and_reaction_identity_fields() {
    let mut runner = QqGatewayMapRunner::new(1, "main");
    let member = Task::new(
        "member",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 3,
            "t": "GROUP_MEMBER_ADD",
            "id": "member-event",
            "d": {
                "group_openid": "GROUP_OPENID",
                "member_openid": "MEMBER_OPENID",
                "timestamp": 1_781_680_853
            }
        }),
    );
    let reaction = Task::new(
        "reaction",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 4,
            "t": "MESSAGE_REACTION_ADD",
            "id": "reaction-event",
            "d": {
                "user_id": "USER_OPENID",
                "group_id": "GROUP_OPENID",
                "target": {"id": "MESSAGE_ID", "type": 0},
                "emoji": {"id": "1", "type": 1}
            }
        }),
    );

    let completion = run_tasks(&mut runner, vec![member, reaction]);
    let events = completion
        .results
        .iter()
        .map(|entry| decode_ingress_event(&entry.result.as_ref().unwrap().tasks[0]))
        .collect::<Vec<_>>();

    assert_eq!(events[0].time_ms, 1_781_680_853_000);
    assert_eq!(events[0].actor.as_ref().unwrap().user_id, "MEMBER_OPENID");
    assert_eq!(events[1].actor.as_ref().unwrap().user_id, "USER_OPENID");
    assert_eq!(
        events[1].target,
        BotTarget::Group {
            group_id: "GROUP_OPENID".into()
        }
    );
}

fn map_gateway_event(event_type: &str, data: Value) -> BotEvent {
    let mut runner = QqGatewayMapRunner::new(1, "main");
    let task = Task::new(
        "frame",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 1,
            "t": event_type,
            "id": "event",
            "d": data,
        }),
    );
    decode_ingress_event(&run_one(&mut runner, task).unwrap().tasks[0])
}

#[test]
fn mentioned_bot_requires_this_bot_not_mention_all() {
    let mention_all = map_gateway_event(
        "GROUP_MESSAGE_CREATE",
        json!({
            "id": "all-message",
            "group_openid": "GROUP_OPENID",
            "content": "@all",
            "author": {"member_openid": "MEMBER_OPENID"}
        }),
    );
    assert_eq!(mention_all.ext["qqbot.mentioned_bot"], Value::Bool(false));
    assert!(
        mention_all
            .message
            .as_ref()
            .unwrap()
            .segments
            .iter()
            .any(|segment| matches!(segment, MessageSegment::MentionAll))
    );

    let group_at = map_gateway_event(
        "GROUP_AT_MESSAGE_CREATE",
        json!({
            "id": "at-message",
            "group_openid": "GROUP_OPENID",
            "content": "hello",
            "author": {"member_openid": "MEMBER_OPENID"}
        }),
    );
    assert_eq!(group_at.ext["qqbot.mentioned_bot"], Value::Bool(true));

    let mention_self = map_gateway_event(
        "GROUP_MESSAGE_CREATE",
        json!({
            "id": "self-message",
            "group_openid": "GROUP_OPENID",
            "content": "<@main> hello",
            "author": {"member_openid": "MEMBER_OPENID"}
        }),
    );
    assert_eq!(mention_self.ext["qqbot.mentioned_bot"], Value::Bool(true));

    let mention_is_you = map_gateway_event(
        "GROUP_MESSAGE_CREATE",
        json!({
            "id": "is-you-message",
            "group_openid": "GROUP_OPENID",
            "content": "hello",
            "mentions": [{"id": "BOT_OPENID", "is_you": true}],
            "author": {"member_openid": "MEMBER_OPENID"}
        }),
    );
    assert_eq!(mention_is_you.ext["qqbot.mentioned_bot"], Value::Bool(true));

    let plain_group = map_gateway_event(
        "GROUP_MESSAGE_CREATE",
        json!({
            "id": "plain-message",
            "group_openid": "GROUP_OPENID",
            "content": "hello",
            "author": {"member_openid": "MEMBER_OPENID"}
        }),
    );
    assert_eq!(plain_group.ext["qqbot.mentioned_bot"], Value::Bool(false));
    assert!(
        !plain_group
            .message
            .as_ref()
            .unwrap()
            .segments
            .iter()
            .any(|segment| matches!(
                segment,
                MessageSegment::MentionUser { .. } | MessageSegment::MentionAll
            ))
    );
}

#[test]
fn gateway_runner_strips_only_the_bot_mention_from_group_at_content() {
    let mut runner = QqGatewayMapRunner::new(1, "main");
    let task = Task::new(
        "group-at",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 5,
            "t": "GROUP_AT_MESSAGE_CREATE",
            "id": "group-at-event",
            "d": {
                "id": "group-at-message",
                "group_openid": "GROUP_OPENID",
                "content": "  &lt;@BOT_OPENID&gt;   /echo hello <@OTHER_USER>  ",
                "mentions": [
                    {"id": "BOT_OPENID", "is_you": true, "bot": true},
                    {"id": "OTHER_USER", "is_you": false, "bot": false}
                ],
                "author": {"member_openid": "MEMBER_OPENID"}
            }
        }),
    );

    let result = run_one(&mut runner, task).unwrap();
    let event = decode_ingress_event(&result.tasks[0]);

    let segments = event.message.unwrap().segments;
    assert_eq!(
        segments
            .iter()
            .filter_map(|segment| match segment {
                MessageSegment::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>(),
        "/echo hello "
    );
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MessageSegment::MentionUser { user_id } if user_id == "OTHER_USER"
    )));
    assert!(!segments.iter().any(|segment| matches!(
        segment,
        MessageSegment::MentionUser { user_id } if user_id == "BOT_OPENID"
    )));
}

#[test]
fn gateway_runner_maps_attachments_ark_markdown_and_keyboard() {
    let mut runner = QqGatewayMapRunner::new(1, "main");
    let task = Task::new(
        "rich",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 8,
            "t": "GROUP_MESSAGE_CREATE",
            "id": "rich-event",
            "d": {
                "id": "rich-message",
                "group_openid": "GROUP_OPENID",
                "content": "look <@member> @all",
                "attachments": [{
                    "url": "http://gchat.qpic.cn/qmeetpic/0",
                    "content_type": "image/png",
                    "filename": "a.png"
                }],
                "ark": {
                    "template_id": 23,
                    "kv": [
                        {"key": "#METATITLE#", "value": "标题"},
                        {"key": "#METADESC#", "value": "描述"}
                    ]
                },
                "markdown": {"content": "**hi**"},
                "keyboard": {"content": {"rows": []}},
                "author": {"member_openid": "MEMBER_OPENID"}
            }
        }),
    );

    let result = run_one(&mut runner, task).unwrap();
    let segments = decode_ingress_event(&result.tasks[0])
        .message
        .unwrap()
        .segments;
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MessageSegment::MentionUser { user_id } if user_id == "member"
    )));
    assert!(
        segments
            .iter()
            .any(|segment| matches!(segment, MessageSegment::MentionAll))
    );
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MessageSegment::PlatformSpecific { platform, kind, payload, .. }
            if platform == "qqbot"
                && kind == "attachment"
                && payload.get("url").and_then(Value::as_str) == Some("https://gchat.qpic.cn/qmeetpic/0")
    )));
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MessageSegment::PlatformSpecific { kind, .. } if kind == "ark"
    )));
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MessageSegment::Markdown { content } if content == "**hi**"
    )));
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MessageSegment::PlatformSpecific { kind, .. } if kind == "keyboard"
    )));
}

#[test]
fn gateway_runner_maps_template_markdown_to_platform_specific() {
    let mut runner = QqGatewayMapRunner::new(1, "main");
    let task = Task::new(
        "template-md",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 8,
            "t": "C2C_MESSAGE_CREATE",
            "id": "template-md-event",
            "d": {
                "id": "template-md-message",
                "author": {"user_openid": "USER_OPENID"},
                "markdown": {
                    "custom_template_id": "tpl-1",
                    "params": [{"key": "title", "values": ["hi"]}]
                }
            }
        }),
    );

    let result = run_one(&mut runner, task).unwrap();
    let segments = decode_ingress_event(&result.tasks[0])
        .message
        .unwrap()
        .segments;
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MessageSegment::PlatformSpecific { platform, kind, payload, .. }
            if platform == "qqbot"
                && kind == "markdown"
                && payload.get("custom_template_id").and_then(Value::as_str) == Some("tpl-1")
    )));
    assert!(
        !segments
            .iter()
            .any(|segment| matches!(segment, MessageSegment::Markdown { .. }))
    );
}

#[test]
fn gateway_runner_maps_face_placeholder_from_image_content() {
    let mut runner = QqGatewayMapRunner::new(1, "main");
    let task = Task::new(
        "image-face",
        QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
        json!({
            "op": 0,
            "s": 9,
            "t": "GROUP_MESSAGE_CREATE",
            "id": "image-face-event",
            "d": {
                "id": "image-face-message",
                "group_openid": "GROUP_OPENID",
                "content": "看看这个<faceType=6,faceId=\"0\",ext=\"eyJ0ZXh0IjoiIn0=\"><@member>",
                "attachments": [{
                    "url": "https://img.example/a.png",
                    "content_type": "image/png",
                    "filename": "a.png"
                }],
                "author": {"member_openid": "MEMBER_OPENID"}
            }
        }),
    );

    let result = run_one(&mut runner, task).unwrap();
    let segments = decode_ingress_event(&result.tasks[0])
        .message
        .unwrap()
        .segments;
    assert_eq!(
        segments
            .iter()
            .filter_map(|segment| match segment {
                MessageSegment::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>(),
        "看看这个"
    );
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MessageSegment::MentionUser { user_id } if user_id == "member"
    )));
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MessageSegment::PlatformSpecific { kind, payload, .. }
            if kind == "face"
                && payload.get("face_type").and_then(|value| value.as_str()) == Some("6")
                && payload.get("face_id").and_then(|value| value.as_str()) == Some("0")
    )));
    assert!(segments.iter().any(|segment| matches!(
        segment,
        MessageSegment::PlatformSpecific { kind, .. } if kind == "attachment"
    )));
}

#[test]
fn gateway_runner_maps_multiple_frames_in_one_batch() {
    let mut runner = QqGatewayMapRunner::new(1, "main");
    let mut tasks = ["first", "second"]
        .into_iter()
        .map(|id| {
            Task::new(
                format!("gateway-{id}"),
                QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
                json!({
                    "op": 0,
                    "s": 24,
                    "t": "C2C_MESSAGE_CREATE",
                    "id": format!("C2C_MESSAGE_CREATE:{id}"),
                    "d": {
                        "id": format!("message-{id}"),
                        "content": id,
                        "author": {"user_openid": "USER_OPENID"}
                    }
                }),
            )
        })
        .collect::<Vec<_>>();
    tasks.insert(
        1,
        Task::new(
            "gateway-invalid-op",
            QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
            json!({"op": 1, "d": {}}),
        ),
    );

    let completion = run_tasks(&mut runner, tasks);

    assert_eq!(completion.results.len(), 3);
    assert!(completion.results[1].result.is_none());
    assert!(completion.results[1].error.is_some());
    for index in [0, 2] {
        let result = completion.results[index].result.as_ref().unwrap();
        assert_eq!(result.tasks.len(), 1);
        assert_eq!(result.tasks[0].registry_generation, 1);
    }
}

#[test]
fn openapi_runner_maps_standard_text_message_to_qqbot_send() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests.clone(),
        vec![
            token_response("TOKEN_A"),
            ok_response(json!({"id": "MESSAGE_ID"})),
        ],
        Box::new(NoopIdSource::new(700)),
    );
    let task = Task::new(
        "send",
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(BotMessage {
            message_id: None,
            target: BotTarget::User {
                user_id: "USER_OPENID".into(),
            },
            sender: None,
            segments: vec![MessageSegment::Text {
                text: "hello".into(),
            }],
            reply_to: Some("SOURCE_MESSAGE_ID".into()),
            time_ms: None,
            ext: Default::default(),
        })
        .unwrap(),
    );

    let result = run_one(&mut runner, task).unwrap();

    assert_eq!(result.events[0].payload["response"]["id"], "MESSAGE_ID");
    let requests = requests.lock().unwrap();
    assert_eq!(requests[1].method, HttpMethod::Post);
    assert_eq!(requests[1].headers["Authorization"], "QQBot TOKEN_A");
    assert_eq!(requests[1].body.as_ref().unwrap()["msg_seq"], 700);
    assert_eq!(requests[1].body.as_ref().unwrap()["content"], "hello");
    assert_eq!(
        requests[1].body.as_ref().unwrap()["msg_id"],
        "SOURCE_MESSAGE_ID"
    );
}

#[test]
fn openapi_runner_sends_group_text_without_msg_id_as_active_message() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests.clone(),
        vec![
            token_response("TOKEN_A"),
            ok_response(json!({"id": "MESSAGE_ID"})),
        ],
        Box::new(NoopIdSource::new(702)),
    );
    let task = Task::new(
        "send-group-active",
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(BotMessage {
            message_id: None,
            target: BotTarget::Group {
                group_id: "GROUP_OPENID".into(),
            },
            sender: None,
            segments: vec![MessageSegment::Text {
                text: "主动推送".into(),
            }],
            reply_to: None,
            time_ms: None,
            ext: Default::default(),
        })
        .unwrap(),
    );

    run_one(&mut runner, task).unwrap();

    let requests = requests.lock().unwrap();
    assert!(
        requests[1]
            .url
            .ends_with("/v2/groups/GROUP_OPENID/messages")
    );
    assert_eq!(requests[1].body.as_ref().unwrap()["msg_type"], 0);
    assert_eq!(requests[1].body.as_ref().unwrap()["content"], "主动推送");
    assert_eq!(requests[1].body.as_ref().unwrap()["msg_seq"], 702);
    assert!(requests[1].body.as_ref().unwrap().get("msg_id").is_none());
}

#[test]
fn openapi_runner_accepts_standard_reply_and_quote_segments() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests.clone(),
        vec![
            token_response("TOKEN_A"),
            ok_response(json!({"id": "MESSAGE_ID"})),
        ],
        Box::new(NoopIdSource::new(701)),
    );
    let task = Task::new(
        "send",
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(BotMessage {
            message_id: None,
            target: BotTarget::User {
                user_id: "USER_OPENID".into(),
            },
            sender: None,
            segments: vec![
                MessageSegment::Reply {
                    message_id: "SOURCE_MESSAGE_ID".into(),
                },
                MessageSegment::Quote {
                    message_id: "SOURCE_MESSAGE_ID".into(),
                },
                MessageSegment::Text {
                    text: "quoted reply".into(),
                },
            ],
            reply_to: None,
            time_ms: None,
            ext: Default::default(),
        })
        .unwrap(),
    );

    run_one(&mut runner, task).unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests[1].body.as_ref().unwrap()["msg_seq"], 701);
    assert_eq!(
        requests[1].body.as_ref().unwrap()["msg_id"],
        "SOURCE_MESSAGE_ID"
    );
    assert_eq!(
        requests[1].body.as_ref().unwrap()["content"],
        "quoted reply"
    );
}

#[test]
fn openapi_runner_rejects_conflicting_reply_and_quote_ids_before_network() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner =
        openapi_runner_with_shared(requests.clone(), Vec::new(), Box::new(NoopIdSource::new(1)));
    let task = Task::new(
        "send",
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(BotMessage {
            message_id: None,
            target: BotTarget::User {
                user_id: "USER_OPENID".into(),
            },
            sender: None,
            segments: vec![
                MessageSegment::Quote {
                    message_id: "OTHER_MESSAGE_ID".into(),
                },
                MessageSegment::Text { text: "no".into() },
            ],
            reply_to: Some("SOURCE_MESSAGE_ID".into()),
            time_ms: None,
            ext: Default::default(),
        })
        .unwrap(),
    );

    let error = run_one(&mut runner, task).unwrap_err();

    assert_eq!(
        error.code,
        mutsuki_bot_protocol::QQBOT_OPENAPI_INVALID_REQUEST_ERROR
    );
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn openapi_runner_preserves_image_then_text_send_order() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests.clone(),
        vec![
            token_response("TOKEN_A"),
            ok_response(json!({"upload_id": "UPLOAD", "block_size": 1024})),
            ok_response(json!({"file_info": "FILE_INFO"})),
            ok_response(json!({"id": "IMAGE_MESSAGE"})),
            ok_response(json!({"id": "TEXT_MESSAGE"})),
        ],
        Box::new(NoopIdSource::new(800)),
    );
    let message = BotMessage {
        message_id: None,
        target: BotTarget::User {
            user_id: "USER_OPENID".into(),
        },
        sender: None,
        segments: vec![
            MessageSegment::Image {
                resource: test_image_resource(),
            },
            MessageSegment::Text {
                text: "caption".into(),
            },
        ],
        reply_to: None,
        time_ms: None,
        ext: Default::default(),
    };
    run_one(
        &mut runner,
        Task::new(
            "image-text",
            BOT_MESSAGE_SEND_PROTOCOL_ID,
            serde_json::to_value(message).unwrap(),
        ),
    )
    .unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests[3].body.as_ref().unwrap()["msg_type"], 7);
    assert_eq!(
        requests[3].body.as_ref().unwrap()["media"]["file_info"],
        "FILE_INFO"
    );
    assert_eq!(requests[4].body.as_ref().unwrap()["content"], "caption");
}

#[test]
fn openapi_runner_sends_audio_video_and_file_through_qq_media_messages() {
    for (segment, file_type) in [
        (
            MessageSegment::Audio {
                resource: test_media_resource("audio", "audio/silk"),
            },
            3,
        ),
        (
            MessageSegment::Video {
                resource: test_media_resource("video", "video/mp4"),
            },
            2,
        ),
        (
            MessageSegment::File {
                resource: test_media_resource("file", "application/octet-stream"),
                name: Some("report.bin".into()),
            },
            4,
        ),
    ] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut runner = openapi_runner_with_shared(
            requests.clone(),
            vec![
                token_response("TOKEN_A"),
                ok_response(json!({"upload_id": "UPLOAD", "block_size": 1024})),
                ok_response(json!({"file_info": "FILE_INFO"})),
                ok_response(json!({"id": "MEDIA_MESSAGE"})),
            ],
            Box::new(NoopIdSource::new(900)),
        );
        let message = BotMessage {
            message_id: None,
            target: BotTarget::Group {
                group_id: "GROUP_OPENID".into(),
            },
            sender: None,
            segments: vec![segment],
            reply_to: Some("SOURCE_MESSAGE_ID".into()),
            time_ms: None,
            ext: Default::default(),
        };
        run_one(
            &mut runner,
            Task::new(
                format!("media-{file_type}"),
                BOT_MESSAGE_SEND_PROTOCOL_ID,
                serde_json::to_value(message).unwrap(),
            ),
        )
        .unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(requests[1].body.as_ref().unwrap()["file_type"], file_type);
        assert_eq!(requests[3].body.as_ref().unwrap()["msg_type"], 7);
        assert_eq!(
            requests[3].body.as_ref().unwrap()["msg_id"],
            "SOURCE_MESSAGE_ID"
        );
    }
}

fn c2c_send_task(task_id: &str, segments: Vec<MessageSegment>) -> Task {
    Task::new(
        task_id,
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(BotMessage {
            message_id: None,
            target: BotTarget::User {
                user_id: "USER_OPENID".into(),
            },
            sender: None,
            segments,
            reply_to: None,
            time_ms: None,
            ext: Default::default(),
        })
        .unwrap(),
    )
}

fn test_image_resource() -> mutsuki_runtime_contracts::ResourceRef {
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };
    mutsuki_runtime_contracts::ResourceRef {
        ref_id: "image-1".into(),
        resource_id: ResourceId {
            kind_id: "image".into(),
            slot_id: "image-1".into(),
            generation: 1,
            version: 1,
        },
        semantic: ResourceSemantic::FrozenValue,
        provider_id: "mutsuki.std.resource.memory".into(),
        resource_kind: "image".into(),
        schema: "mutsuki.bot.image.original.v1".into(),
        version: 1,
        generation: 1,
        access: ResourceAccess::ProviderRpc {
            provider_id: "mutsuki.std.resource.memory".into(),
            method: "memory".into(),
        },
        size_hint: Some(3),
        content_hash: Some("sha256:image".into()),
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

fn test_media_resource(id: &str, schema: &str) -> mutsuki_runtime_contracts::ResourceRef {
    let mut resource = test_image_resource();
    resource.ref_id = format!("{id}-1").into();
    resource.resource_id.kind_id = id.into();
    resource.resource_id.slot_id = resource.ref_id.to_string();
    resource.resource_kind = id.into();
    resource.schema = schema.into();
    resource.content_hash = Some(format!("sha256:{id}"));
    resource
}

#[test]
fn openapi_runner_maps_standard_recall_to_qqbot_delete() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests.clone(),
        vec![token_response("TOKEN_A"), ok_response(json!({"ok": true}))],
        Box::new(NoopIdSource::new(1)),
    );
    let task = Task::new(
        "recall",
        BOT_MESSAGE_RECALL_PROTOCOL_ID,
        serde_json::to_value(BotMessageRecallRequest {
            target: BotTarget::Group {
                group_id: "GROUP_OPENID".into(),
            },
            message_id: "MESSAGE_ID".into(),
        })
        .unwrap(),
    );

    let result = run_one(&mut runner, task).unwrap();

    assert_eq!(result.events[0].payload["response"]["ok"], true);
    let requests = requests.lock().unwrap();
    assert_eq!(requests[1].method, HttpMethod::Delete);
    assert!(
        requests[1]
            .url
            .ends_with("/v2/groups/GROUP_OPENID/messages/MESSAGE_ID")
    );
    assert_eq!(requests[1].body.as_ref(), Some(&Value::Null));
}

#[test]
fn openapi_runner_gets_qqbot_account_from_openapi() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests.clone(),
        vec![
            token_response("TOKEN_A"),
            ok_response(json!({"id": "BOT_OPENID", "username": "mutsuki"})),
        ],
        Box::new(NoopIdSource::new(1)),
    );
    let task = Task::new("account", QQBOT_ACCOUNT_GET_PROTOCOL_ID, json!({}));

    let result = run_one(&mut runner, task).unwrap();

    let response = &result.events[0].payload["response"];
    assert_eq!(response["account"]["account_id"], "main");
    assert_eq!(response["account"]["platform"], "qqbot");
    assert_eq!(response["app_id"], "APP_ID");
    assert_eq!(response["openapi_user"]["id"], "BOT_OPENID");
    assert_eq!(response["user"]["user_id"], "BOT_OPENID");
    assert_eq!(response["user"]["display_name"], "mutsuki");
    assert_eq!(
        response["user"]["avatar_url"],
        "https://q.qlogo.cn/qqapp/APP_ID/BOT_OPENID/640"
    );
    let requests = requests.lock().unwrap();
    assert_eq!(requests[1].method, HttpMethod::Get);
    assert!(requests[1].url.ends_with("/users/@me"));
    assert_eq!(requests[1].body, None);
    assert_eq!(requests[1].headers["Authorization"], "QQBot TOKEN_A");
}

#[test]
fn openapi_runner_gets_gateway_status_from_openapi() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests.clone(),
        vec![
            token_response("TOKEN_A"),
            ok_response(json!({"url": "wss://gateway.example.invalid"})),
        ],
        Box::new(NoopIdSource::new(1)),
    );
    let task = Task::new(
        "gateway-status",
        QQBOT_GATEWAY_STATUS_PROTOCOL_ID,
        json!({}),
    );

    let result = run_one(&mut runner, task).unwrap();

    let response = &result.events[0].payload["response"];
    assert_eq!(response["account_id"], "main");
    assert_eq!(response["platform"], "qqbot");
    assert_eq!(response["gateway"]["url"], "wss://gateway.example.invalid");
    assert_eq!(response["shard"], json!([0, 1]));
    assert_eq!(response["intents"], DEFAULT_QQBOT_INTENTS);
    let requests = requests.lock().unwrap();
    assert_eq!(requests[1].method, HttpMethod::Get);
    assert!(requests[1].url.ends_with("/gateway/bot"));
    assert_eq!(requests[1].body, None);
    assert_eq!(requests[1].headers["Authorization"], "QQBot TOKEN_A");
}

#[test]
fn openapi_descriptor_accepts_manifest_provided_qqbot_protocols() {
    let descriptor = openapi_descriptor(1, true);

    assert!(
        descriptor
            .accepted_protocol_ids
            .contains(&QQBOT_ACCOUNT_GET_PROTOCOL_ID.into())
    );
    assert!(
        descriptor
            .accepted_protocol_ids
            .contains(&QQBOT_GATEWAY_STATUS_PROTOCOL_ID.into())
    );
    assert_eq!(descriptor.batch.max_entry_concurrency, 1);
    assert_eq!(descriptor.batch.side_effect, RunnerSideEffect::External);
    assert!(descriptor.batch.preserve_order);
    assert_eq!(
        descriptor.ordering.default,
        OrderingRequirement::PreserveSubmitOrder
    );
}

#[test]
fn text_only_descriptor_does_not_claim_media_upload() {
    let descriptor = openapi_descriptor(1, false);
    assert!(
        !descriptor
            .accepted_protocol_ids
            .contains(&BOT_MEDIA_UPLOAD_PROTOCOL_ID.into())
    );
    let manifest = qqbot_adapter_manifest(1, false);
    assert!(
        manifest
            .provides
            .protocols
            .iter()
            .all(|protocol| protocol.protocol_id != BOT_MEDIA_UPLOAD_PROTOCOL_ID)
    );
}

#[test]
fn qq_sources_are_split_by_received_kind() {
    use mutsuki_bot_protocol::{
        BOT_FLOW_MESSAGE_EVENT_TYPE, BOT_FLOW_NODE_EXTENSION_ID, BotNodeCatalogFragment,
    };

    let manifest = qqbot_adapter_manifest(1, false);
    let fragment = manifest
        .provides
        .extensions
        .iter()
        .find(|extension| extension.extension_id == BOT_FLOW_NODE_EXTENSION_ID)
        .and_then(|extension| {
            BotNodeCatalogFragment::from_plugin_extension(extension)
                .ok()
                .flatten()
        })
        .expect("QQ node catalog");
    let titles = fragment
        .nodes
        .iter()
        .filter(|node| node.role == mutsuki_bot_protocol::BotNodeRole::Source)
        .map(|node| node.title.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        titles,
        [
            "收到消息",
            "消息更新",
            "消息删除",
            "添加表情",
            "取消表情",
            "成员加入",
            "成员离开",
            "机器人上线",
            "机器人下线",
            "平台事件",
        ]
    );
    let message = fragment
        .nodes
        .iter()
        .find(|node| node.node_type_id == crate::QQ_NODE_MESSAGE_CREATED)
        .unwrap();
    assert_eq!(
        message.ports[0].event_type.type_id,
        BOT_FLOW_MESSAGE_EVENT_TYPE
    );
    assert!(
        !fragment
            .nodes
            .iter()
            .any(|node| node.node_type_id == "mutsuki.bot.qq.source")
    );
}

#[test]
fn qq_forward_fold_binds_openapi_runner() {
    use mutsuki_bot_protocol::{BOT_FLOW_NODE_EXTENSION_ID, BotNodeCatalogFragment};

    let descriptor = openapi_descriptor(1, false);
    assert!(
        descriptor
            .accepted_protocol_ids
            .contains(&BOT_QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID.into())
    );

    let manifest = qqbot_adapter_manifest(1, false);
    assert!(
        manifest
            .provides
            .protocols
            .iter()
            .any(|protocol| protocol.protocol_id == BOT_QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID)
    );
    let binding = manifest
        .provides
        .handler_bindings
        .iter()
        .find(|binding| binding.protocol_id == BOT_QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID)
        .expect("forward-fold handler binding");
    assert_eq!(
        binding.target_runner_hint.as_deref(),
        Some(QQBOT_OPENAPI_RUNNER_ID)
    );

    let fragment = manifest
        .provides
        .extensions
        .iter()
        .find(|extension| extension.extension_id == BOT_FLOW_NODE_EXTENSION_ID)
        .and_then(|extension| {
            BotNodeCatalogFragment::from_plugin_extension(extension)
                .ok()
                .flatten()
        })
        .expect("QQ node catalog");
    let node = fragment
        .nodes
        .iter()
        .find(|node| node.node_type_id == "mutsuki.bot.qq.reply.forward_fold")
        .expect("forward-fold catalog node");
    let catalog_binding = node.binding.as_ref().expect("catalog binding");
    assert_eq!(
        catalog_binding.protocol_id,
        BOT_QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID
    );
    assert_eq!(
        catalog_binding.runner_hint.as_deref(),
        Some(QQBOT_OPENAPI_RUNNER_ID)
    );
}

#[test]
fn apply_forward_fold_collapses_long_text_to_qq_forward() {
    let mut request = reply_request(&"字".repeat(20));
    apply_forward_fold(&json!({"threshold": 10}), &mut request).unwrap();
    assert_eq!(request.parts.len(), 1);
    assert_eq!(
        request.parts[0].content.segments[0],
        MessageSegment::platform_specific("qqbot", "forward", json!({ "text": "字".repeat(20) }))
    );
    assert_eq!(
        request.parts[0].content.summary.as_deref(),
        Some("字".repeat(20).as_str())
    );
}

#[test]
fn apply_forward_fold_leaves_short_text_unchanged() {
    let mut request = reply_request("short");
    apply_forward_fold(&json!({"threshold": 10}), &mut request).unwrap();
    assert_eq!(
        request.parts[0].content.segments,
        vec![MessageSegment::text("short")]
    );
}

#[test]
fn openapi_runner_applies_forward_fold_without_http() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner =
        openapi_runner_with_shared(requests.clone(), vec![], Box::new(NoopIdSource::new(1)));
    let request = reply_request(&"长".repeat(12));
    let invocation = BotNodeInvocation {
        flow_id: "flow".into(),
        graph_revision: 1,
        execution_id: "ex".into(),
        node_id: "n".into(),
        input_port_id: "reply".into(),
        config: json!({"threshold": 8}),
        input: BotFlowEventEnvelope {
            event_id: "e1".into(),
            protocol_id: "mutsuki.bot.delivery/reply@1".into(),
            payload: BotFlowPayload {
                event_type: BotFlowTypeRef::new(BOT_FLOW_DELIVERY_REPLY_TYPE, 1),
                value: serde_json::to_value(&request).unwrap(),
            },
            context: BotFlowContext {
                bot: None,
                target: None,
                actor: None,
                ext: BTreeMap::new(),
            },
            trace_id: None,
            correlation_id: None,
        },
    };
    let task = Task::new(
        "fold-task",
        BOT_QQ_REPLY_FORWARD_FOLD_PROTOCOL_ID,
        serde_json::to_value(&invocation).unwrap(),
    );
    let result = run_one(&mut runner, task).unwrap();
    assert!(requests.lock().unwrap().is_empty());
    let node: BotNodeResult = serde_json::from_value(result.output.unwrap()).unwrap();
    let folded: BotReplyDeliveryRequest =
        serde_json::from_value(node.outputs[0].event.payload.value.clone()).unwrap();
    assert!(matches!(
        folded.parts[0].content.segments[0],
        MessageSegment::PlatformSpecific { ref platform, ref kind, .. }
            if platform == "qqbot" && kind == "forward"
    ));
}

fn reply_request(text: &str) -> BotReplyDeliveryRequest {
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

#[test]
fn qqbot_config_deserializes_defaults_and_rejects_unknown_fields() {
    let config: QqBotConfig = serde_json::from_value(json!({
        "account_id": "main",
        "app_id": "APP_ID",
        "client_secret_key": "QQBOT_SECRET"
    }))
    .unwrap();
    assert_eq!(config.openapi_base_url, "https://api.sgroup.qq.com");
    assert!(config.validate().is_ok());
    assert!(
        serde_json::from_value::<QqBotConfig>(json!({
            "account_id": "main",
            "app_id": "APP_ID",
            "raw_secret": "forbidden"
        }))
        .is_err()
    );
}

#[test]
fn openapi_batch_isolates_unsupported_protocol_and_traces_success_event() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests,
        vec![
            token_response("TOKEN_A"),
            ok_response(json!({"id": "BOT_OPENID"})),
        ],
        Box::new(NoopIdSource::new(1)),
    );
    let unsupported = Task::new("unsupported", "mutsuki.bot.unsupported@1", json!({}));
    let account = Task::new("account", QQBOT_ACCOUNT_GET_PROTOCOL_ID, json!({}));

    let completion = run_tasks(&mut runner, vec![unsupported, account]);

    assert!(completion.results[0].result.is_none());
    assert!(completion.results[0].error.is_some());
    let event = &completion.results[1].result.as_ref().unwrap().events[0];
    assert_eq!(event.event_id, "account:result");
    assert_eq!(event.payload["task_id"], "account");
    assert_eq!(event.payload["protocol_id"], QQBOT_ACCOUNT_GET_PROTOCOL_ID);
}

#[test]
fn openapi_batch_isolates_api_failure_and_continues_in_order() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests,
        vec![
            token_response("TOKEN_A"),
            Err(QqOpenApiError::HttpStatus {
                status: 400,
                headers: BTreeMap::new(),
                body: json!({"message": "invalid send"}),
            }),
            ok_response(json!({"id": "BOT_OPENID"})),
        ],
        Box::new(NoopIdSource::new(1)),
    );
    let send = Task::new(
        "send",
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(BotMessage::text(
            BotTarget::User {
                user_id: "USER_OPENID".into(),
            },
            "hello",
        ))
        .unwrap(),
    );
    let account = Task::new("account", QQBOT_ACCOUNT_GET_PROTOCOL_ID, json!({}));

    let completion = run_tasks(&mut runner, vec![send, account]);

    assert!(completion.results[0].result.is_none());
    let error = completion.results[0].error.as_ref().unwrap();
    assert_eq!(error.code, QQBOT_OPENAPI_PERMANENT_ERROR);
    assert_eq!(
        error.evidence.get("classification"),
        Some(&mutsuki_runtime_contracts::ScalarValue::String(
            "permanent".into()
        ))
    );
    assert_eq!(
        error.evidence.get("retryable"),
        Some(&mutsuki_runtime_contracts::ScalarValue::Bool(false))
    );
    assert!(completion.results[1].error.is_none());
    assert_eq!(
        completion.results[1].result.as_ref().unwrap().events[0].payload["task_id"],
        "account"
    );
}

#[test]
fn openapi_task_exposes_rate_limit_and_retry_after() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut config = QqBotConfig::new("main", "APP_ID");
    config.max_retry_attempts = 1;
    config.retry_base_delay_ms = 0;
    config.retry_max_delay_ms = 0;
    let mut runner = openapi_runner_with_config(
        config,
        requests,
        vec![
            token_response("TOKEN_A"),
            Ok(QqHttpResponse {
                status: 429,
                headers: BTreeMap::from([("Retry-After".into(), "0.25".into())]),
                body: json!({"message": "slow down"}),
            }),
        ],
        Box::new(NoopIdSource::new(1)),
    );

    let completion = run_tasks(
        &mut runner,
        vec![Task::new(
            "account",
            QQBOT_ACCOUNT_GET_PROTOCOL_ID,
            json!({}),
        )],
    );

    let error = completion.results[0].error.as_ref().unwrap();
    assert_eq!(error.code, QQBOT_OPENAPI_RATE_LIMITED_ERROR);
    assert_eq!(error.recovery.as_deref(), Some("retry"));
    assert_eq!(
        error.evidence.get("classification"),
        Some(&mutsuki_runtime_contracts::ScalarValue::String(
            "rate_limited".into()
        ))
    );
    assert_eq!(
        error.evidence.get("retry_after_ms"),
        Some(&mutsuki_runtime_contracts::ScalarValue::Int(250))
    );
}

#[test]
fn openapi_runner_rejects_raw_call_absolute_url_without_request() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner =
        openapi_runner_with_shared(requests.clone(), Vec::new(), Box::new(NoopIdSource::new(1)));
    let task = Task::new(
        "raw-call",
        QQBOT_RAW_CALL_PROTOCOL_ID,
        json!({
            "method": "POST",
            "path": "https://example.invalid/steal",
            "body": {}
        }),
    );

    let result = run_one(&mut runner, task);

    assert!(result.is_err());
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn openapi_runner_rejects_qqbot_raw_body_in_standard_send() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner =
        openapi_runner_with_shared(requests.clone(), Vec::new(), Box::new(NoopIdSource::new(1)));
    let task = Task::new(
        "send",
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(BotMessage {
            message_id: None,
            target: BotTarget::User {
                user_id: "USER_OPENID".into(),
            },
            sender: None,
            segments: vec![MessageSegment::PlatformSpecific {
                platform: "qqbot".into(),
                kind: "message_body".into(),
                payload: json!({"msg_type": 0, "content": "raw"}),
            }],
            reply_to: None,
            time_ms: None,
            ext: Default::default(),
        })
        .unwrap(),
    );

    let result = run_one(&mut runner, task);

    assert!(result.is_err());
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn openapi_runner_sends_markdown_and_keyboard_in_one_body() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests.clone(),
        vec![
            token_response("TOKEN_A"),
            ok_response(json!({"id": "MESSAGE_ID"})),
        ],
        Box::new(NoopIdSource::new(901)),
    );

    run_one(
        &mut runner,
        c2c_send_task(
            "send-md-keyboard",
            vec![
                MessageSegment::markdown("# title"),
                MessageSegment::platform_specific(
                    "qqbot",
                    "keyboard",
                    json!({"content": {"rows": []}}),
                ),
            ],
        ),
    )
    .unwrap();

    let requests = requests.lock().unwrap();
    let body = requests[1].body.as_ref().unwrap();
    assert_eq!(body["msg_type"], 2);
    assert_eq!(body["markdown"]["content"], "# title");
    assert_eq!(body["keyboard"]["content"]["rows"], json!([]));
    assert!(body.get("content").is_none());
}

#[test]
fn openapi_runner_splits_markdown_then_image_into_two_sends() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests.clone(),
        vec![
            token_response("TOKEN_A"),
            ok_response(json!({"id": "MD_MESSAGE"})),
            ok_response(json!({"upload_id": "UPLOAD", "block_size": 1024})),
            ok_response(json!({"file_info": "FILE_INFO"})),
            ok_response(json!({"id": "IMAGE_MESSAGE"})),
        ],
        Box::new(NoopIdSource::new(902)),
    );

    run_one(
        &mut runner,
        c2c_send_task(
            "md-image",
            vec![
                MessageSegment::markdown("# pic"),
                MessageSegment::Image {
                    resource: test_image_resource(),
                },
            ],
        ),
    )
    .unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests[1].body.as_ref().unwrap()["msg_type"], 2);
    assert_eq!(
        requests[1].body.as_ref().unwrap()["markdown"]["content"],
        "# pic"
    );
    assert_eq!(requests[4].body.as_ref().unwrap()["msg_type"], 7);
}

#[test]
fn auth_uses_wall_clock_expiry_and_refreshes_after_real_seconds() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut client = FakeHttpClient {
        requests: requests.clone(),
        responses: Mutex::new(VecDeque::from([
            token_response("TOKEN_A"),
            token_response("TOKEN_B"),
        ])),
    };
    let config = QqBotConfig::new("main", "APP_ID");
    let credentials = StaticQqCredentials::new("CLIENT_SECRET");
    let auth = QqAuthManager::new();

    assert_eq!(
        auth.bearer_token_at(&config, &credentials, &mut client, 1_000)
            .unwrap(),
        "TOKEN_A"
    );
    assert_eq!(
        auth.bearer_token_at(&config, &credentials, &mut client, 2_000)
            .unwrap(),
        "TOKEN_A"
    );
    assert_eq!(
        auth.bearer_token_at(&config, &credentials, &mut client, 8_100)
            .unwrap(),
        "TOKEN_B"
    );
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[test]
fn auth_accepts_numeric_expires_in() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut client = FakeHttpClient {
        requests,
        responses: Mutex::new(VecDeque::from([ok_response(json!({
            "access_token": "TOKEN_A",
            "expires_in": 7200
        }))])),
    };
    let config = QqBotConfig::new("main", "APP_ID");
    let credentials = StaticQqCredentials::new("CLIENT_SECRET");

    assert_eq!(
        QqAuthManager::new()
            .bearer_token_at(&config, &credentials, &mut client, 1_000)
            .unwrap(),
        "TOKEN_A"
    );
}

#[test]
fn transport_retries_429_and_5xx_with_bounded_attempts() {
    for status in [429, 503] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut config = QqBotConfig::new("main", "APP_ID");
        config.max_retry_attempts = 2;
        config.retry_base_delay_ms = 0;
        config.retry_max_delay_ms = 0;
        let client = FakeHttpClient {
            requests: requests.clone(),
            responses: Mutex::new(VecDeque::from([
                token_response("TOKEN_A"),
                Ok(QqHttpResponse {
                    status,
                    headers: BTreeMap::from([("Retry-After".into(), "0".into())]),
                    body: json!({"message": "retry"}),
                }),
                ok_response(json!({"ok": true})),
            ])),
        };
        let mut transport = QqOpenApiTransport::new(
            config,
            Box::new(client),
            Arc::new(StaticQqCredentials::new("CLIENT_SECRET")),
        );

        assert_eq!(
            transport
                .execute_json(HttpMethod::Get, "/users/@me".into(), Value::Null)
                .unwrap()["ok"],
            true
        );
        assert_eq!(requests.lock().unwrap().len(), 3);
    }
}

#[test]
fn transport_honors_single_attempt_for_5xx_while_preserving_401_refresh() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut config = QqBotConfig::new("main", "APP_ID");
    config.max_retry_attempts = 1;
    config.retry_base_delay_ms = 0;
    config.retry_max_delay_ms = 0;
    let client = FakeHttpClient {
        requests: requests.clone(),
        responses: Mutex::new(VecDeque::from([
            token_response("TOKEN_A"),
            Ok(QqHttpResponse {
                status: 503,
                headers: BTreeMap::new(),
                body: json!({"message": "do not retry"}),
            }),
        ])),
    };
    let mut transport = QqOpenApiTransport::new(
        config,
        Box::new(client),
        Arc::new(StaticQqCredentials::new("CLIENT_SECRET")),
    );

    let error = transport
        .execute_json(HttpMethod::Get, "/users/@me".into(), Value::Null)
        .unwrap_err();

    assert!(matches!(
        error,
        QqOpenApiError::HttpStatus { status: 503, .. }
    ));
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[test]
fn transport_refreshes_only_once_for_repeated_401() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut config = QqBotConfig::new("main", "APP_ID");
    config.max_retry_attempts = 4;
    let unauthorized = || {
        Ok(QqHttpResponse {
            status: 401,
            headers: BTreeMap::new(),
            body: json!({"message": "unauthorized"}),
        })
    };
    let client = FakeHttpClient {
        requests: requests.clone(),
        responses: Mutex::new(VecDeque::from([
            token_response("TOKEN_A"),
            unauthorized(),
            token_response("TOKEN_B"),
            unauthorized(),
        ])),
    };
    let mut transport = QqOpenApiTransport::new(
        config,
        Box::new(client),
        Arc::new(StaticQqCredentials::new("CLIENT_SECRET")),
    );

    let error = transport
        .execute_json(HttpMethod::Get, "/users/@me".into(), Value::Null)
        .unwrap_err();
    assert!(matches!(
        error,
        QqOpenApiError::HttpStatus { status: 401, .. }
    ));
    assert_eq!(requests.lock().unwrap().len(), 4);
}

#[test]
fn config_rejects_insecure_or_credential_bearing_urls() {
    let mut config = QqBotConfig::new("main", "APP_ID");
    config.token_url = "http://bots.qq.com/token".into();
    assert!(config.validate().is_err());
    config.token_url = "https://user:password@bots.qq.com/token".into();
    assert!(config.validate().is_err());
    config.token_url = "https://bots.qq.com/token".into();
    config.openapi_base_url = "not-a-url".into();
    assert!(config.validate().is_err());
    assert!(
        crate::config::validate_gateway_url("wss://user:password@gateway.example", false).is_err()
    );

    let mut invalid_intents = QqBotConfig::new("main", "APP_ID");
    invalid_intents.gateway_intents = 0;
    assert!(invalid_intents.validate().is_err());

    let mut invalid_shard = QqBotConfig::new("main", "APP_ID");
    invalid_shard.shard = [1, 1];
    assert!(invalid_shard.validate().is_err());
}

#[test]
fn errors_redact_secret_token_authorization_and_openid() {
    let error = QqOpenApiError::HttpStatus {
        status: 400,
        headers: BTreeMap::from([("Authorization".into(), "QQBot SECRET_TOKEN".into())]),
        body: json!({
            "clientSecret": "CLIENT_SECRET",
            "access_token": "ACCESS_TOKEN",
            "user_openid": "USER_OPENID"
        }),
    };
    let message = error.redacted_message();
    assert!(!message.contains("CLIENT_SECRET"));
    assert!(!message.contains("ACCESS_TOKEN"));
    assert!(!message.contains("USER_OPENID"));
    assert!(!message.contains("SECRET_TOKEN"));
    assert!(message.contains("<redacted>"));
    assert_eq!(error.to_string(), "http status 400");
    let error_debug = format!("{error:?}");
    assert!(!error_debug.contains("CLIENT_SECRET"));
    assert!(!error_debug.contains("ACCESS_TOKEN"));

    let request_debug = format!(
        "{:?}",
        QqHttpRequest {
            method: HttpMethod::Post,
            url: "https://api.example/v2/users/USER_OPENID/messages?signature=SECRET_SIGNATURE"
                .into(),
            headers: BTreeMap::from([("Authorization".into(), "QQBot SECRET_TOKEN".into(),)]),
            body: Some(json!({"clientSecret": "CLIENT_SECRET"})),
            binary_body: None,
        }
    );
    for secret in [
        "USER_OPENID",
        "SECRET_SIGNATURE",
        "SECRET_TOKEN",
        "CLIENT_SECRET",
    ] {
        assert!(!request_debug.contains(secret));
    }
    let transport_error = crate::adapter::redact_urls(
        "request failed for https://api.example/path?signature=SECRET_SIGNATURE",
    );
    assert!(!transport_error.contains("SECRET_SIGNATURE"));
}

#[test]
fn gateway_pump_models_identify_heartbeat_resume_and_reconnect() {
    let mut pump = QqGatewayPump::with_account("main", 8);
    pump.handle_raw_frame(json!({"op": 10, "d": {"heartbeat_interval": 1000}}), 0)
        .unwrap();
    assert_eq!(pump.pop_action(), Some(GatewayAction::Identify));

    pump.handle_raw_frame(
        json!({
            "op": 0,
            "s": 1,
            "t": "READY",
            "id": "ready-1",
            "d": {"session_id": "SESSION", "resume_gateway_url": "wss://resume.example"}
        }),
        0,
    )
    .unwrap();
    assert_eq!(pump.session_id(), Some("SESSION"));
    assert_eq!(pump.resume_url(), Some("wss://resume.example"));
    let _ = pump.pop_action();

    pump.handle_raw_frame(json!({"op": 10, "d": {"heartbeat_interval": 1000}}), 0)
        .unwrap();
    assert_eq!(pump.pop_action(), Some(GatewayAction::Resume));
    assert_eq!(pump.heartbeat_frame(), json!({"op": 1, "d": 1}));
    assert_eq!(pump.heartbeat_text(), r#"{"op":1,"d":1}"#);

    pump.handle_raw_frame(json!({"op": 11, "d": null}), 0)
        .unwrap();
    assert_eq!(pump.pop_action(), Some(GatewayAction::AckHeartbeat));
    pump.handle_raw_frame(json!({"op": 7, "d": null}), 0)
        .unwrap();
    assert_eq!(pump.pop_action(), Some(GatewayAction::Reconnect));
}

#[test]
fn gateway_pump_invalid_session_resumes_or_reidentifies() {
    let mut pump = QqGatewayPump::with_account("main", 8);
    pump.handle_raw_frame(json!({"op": 9, "d": false}), 0)
        .unwrap();
    assert_eq!(pump.pop_action(), Some(GatewayAction::Identify));
    assert_eq!(pump.session_id(), None);

    pump.handle_raw_frame(
        json!({
            "op": 0,
            "s": 1,
            "t": "READY",
            "id": "ready-1",
            "d": {"session_id": "SESSION", "resume_gateway_url": "wss://resume.example"}
        }),
        0,
    )
    .unwrap();
    let _ = pump.pop_action();
    pump.handle_raw_frame(json!({"op": 9, "d": true}), 0)
        .unwrap();
    assert_eq!(pump.pop_action(), Some(GatewayAction::Resume));
    assert_eq!(pump.session_id(), Some("SESSION"));
}

#[test]
fn gateway_pump_bounds_dedup_window_and_tolerates_unknown_frames() {
    let mut pump = QqGatewayPump::with_account("main", 2);
    let frame = |id: &str, sequence: u64| {
        json!({
            "op": 0,
            "s": sequence,
            "t": "C2C_MESSAGE_CREATE",
            "id": format!("event-{id}"),
            "d": {"id": id, "content": "hello"}
        })
    };
    for (id, sequence) in [("one", 1), ("two", 2), ("three", 3), ("one", 4)] {
        assert!(
            pump.handle_raw_frame(frame(id, sequence), 0)
                .unwrap()
                .is_some()
        );
        assert!(matches!(
            pump.pop_action(),
            Some(GatewayAction::DispatchTask(_))
        ));
    }

    assert!(
        pump.handle_raw_frame(json!({"op": 99, "d": {}}), 0)
            .unwrap()
            .is_none()
    );
    assert_eq!(pump.pop_action(), Some(GatewayAction::UnknownOpcode(99)));
    assert!(
        pump.handle_raw_frame(
            json!({"op": 0, "s": 5, "t": "FUTURE_EVENT", "id": "future", "d": {}}),
            0,
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(
        pump.pop_action(),
        Some(GatewayAction::UnknownEvent("FUTURE_EVENT".into()))
    );
}

#[test]
fn gateway_pump_ingests_default_intent_events_previously_dropped() {
    let mut pump = QqGatewayPump::with_account("main", 8);
    let mut runner = QqGatewayMapRunner::new(1, "main");
    for (sequence, event_type) in [(40, "GUILD_CREATE"), (41, "MESSAGE_AUDIT_PASS")] {
        let task = pump
            .handle_raw_frame(
                json!({
                    "op": 0,
                    "s": sequence,
                    "t": event_type,
                    "id": format!("event-{sequence}"),
                    "d": {"id": format!("payload-{sequence}"), "guild_id": "guild"}
                }),
                1,
            )
            .unwrap()
            .unwrap();
        let event = decode_ingress_event(&run_one(&mut runner, task).unwrap().tasks[0]);
        assert_eq!(
            event.kind,
            BotEventKind::PlatformSpecific(event_type.into())
        );
    }
}

#[test]
fn gateway_pump_releases_dedup_reservation_after_submit_rejection() {
    let mut pump = QqGatewayPump::with_account("main", 8);
    let raw = json!({
        "op": 0,
        "s": 1,
        "t": "C2C_MESSAGE_CREATE",
        "id": "event-one",
        "d": {"id": "message-one", "content": "hello"}
    });
    let frame: crate::gateway::GatewayFrame = serde_json::from_value(raw.clone()).unwrap();

    assert!(pump.handle_raw_frame(raw.clone(), 0).unwrap().is_some());
    pump.forget_dispatch(&frame);
    assert!(pump.handle_raw_frame(raw, 0).unwrap().is_some());
}

#[test]
fn gateway_dedup_keeps_distinct_lifecycle_events_for_the_same_message() {
    let mut pump = QqGatewayPump::with_account("main", 8);
    let frame = |event_type: &str, event_id: &str, sequence: u64| {
        json!({
            "op": 0,
            "s": sequence,
            "t": event_type,
            "id": event_id,
            "d": {
                "id": "message-one",
                "guild_id": "guild",
                "channel_id": "channel"
            }
        })
    };

    assert!(
        pump.handle_raw_frame(frame("AT_MESSAGE_CREATE", "create", 1), 0)
            .unwrap()
            .is_some()
    );
    let _ = pump.pop_action();
    assert!(
        pump.handle_raw_frame(frame("PUBLIC_MESSAGE_DELETE", "delete", 2), 0)
            .unwrap()
            .is_some()
    );
}

fn openapi_runner_with_shared(
    requests: Arc<Mutex<Vec<QqHttpRequest>>>,
    responses: Vec<Result<QqHttpResponse, QqOpenApiError>>,
    id_source: Box<dyn QqIdSource>,
) -> QqOpenApiRunner {
    let config = QqBotConfig::new("main", "APP_ID");
    openapi_runner_with_config(config, requests, responses, id_source)
}

fn openapi_runner_with_config(
    config: QqBotConfig,
    requests: Arc<Mutex<Vec<QqHttpRequest>>>,
    responses: Vec<Result<QqHttpResponse, QqOpenApiError>>,
    id_source: Box<dyn QqIdSource>,
) -> QqOpenApiRunner {
    let clients = QqBotClients::new(
        Box::new(FakeHttpClient {
            requests,
            responses: Mutex::new(VecDeque::from(responses)),
        }),
        Arc::new(StaticQqCredentials::new("CLIENT_SECRET")),
    )
    .with_media_provider(Box::new(FakeMediaProvider));
    QqOpenApiRunner::new(1, config, clients, id_source)
}

fn token_response(token: &str) -> Result<QqHttpResponse, QqOpenApiError> {
    ok_response(json!({"access_token": token, "expires_in": "7200"}))
}

fn ok_response(body: Value) -> Result<QqHttpResponse, QqOpenApiError> {
    Ok(QqHttpResponse {
        status: 200,
        headers: BTreeMap::new(),
        body,
    })
}

struct FakeHttpClient {
    requests: Arc<Mutex<Vec<QqHttpRequest>>>,
    responses: Mutex<VecDeque<Result<QqHttpResponse, QqOpenApiError>>>,
}

impl QqHttpClient for FakeHttpClient {
    fn send(&mut self, request: QqHttpRequest) -> Result<QqHttpResponse, QqOpenApiError> {
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("missing fake HTTP response")
    }
}

struct FakeMediaProvider;

impl QqMediaProvider for FakeMediaProvider {
    fn read_chunks(
        &mut self,
        _resource_ref: &mutsuki_runtime_contracts::ResourceRef,
        _block_size: u64,
    ) -> Result<Vec<MediaChunk>, QqMediaError> {
        Ok(Vec::new())
    }
}

struct NoopIdSource {
    next: u64,
}

impl NoopIdSource {
    fn new(next: u64) -> Self {
        Self { next }
    }
}

impl QqIdSource for NoopIdSource {
    fn next_msg_seq(&mut self) -> u64 {
        let next = self.next;
        self.next += 1;
        next
    }
}

fn test_context(current_step: u64) -> RunnerContext {
    RunnerContext::new(
        1,
        current_step,
        "executor:test",
        Some("task-lease-test"),
        "invocation:test",
    )
}

fn run_one(runner: &mut impl Runner, task: Task) -> Result<RunnerResult, RuntimeError> {
    let completion = run_tasks(runner, vec![task]);
    let entry = completion
        .results
        .into_iter()
        .next()
        .expect("single-entry batch completion");
    match (entry.result, entry.error) {
        (Some(result), None) => Ok(result),
        (None, Some(error)) => Err(error),
        _ => panic!("entry completion must contain exactly one outcome"),
    }
}

fn run_tasks(runner: &mut impl Runner, tasks: Vec<Task>) -> CompletionBatch {
    let entries = tasks
        .iter()
        .enumerate()
        .map(|(index, task)| BatchEntry {
            entry_id: format!("entry-{index}").into(),
            task_id: task.task_id.clone(),
            trace_id: task.trace_id.clone(),
            parent_id: None,
            payload_index: index,
            resource_requirement_indices: Vec::new(),
            cancel_index: None,
            deadline_tick: None,
            priority: 0,
            lane: DispatchLane::Normal,
            ordering: OrderingRequirement::PreserveSubmitOrder,
        })
        .collect();
    let batch = WorkBatch {
        batch_id: "batch:test".into(),
        tick_id: "tick:test".into(),
        batch_key: BatchKey::from(runner.descriptor().runner_id.as_str()),
        entries,
        payload: BatchPayload::from_tasks(&tasks),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: Vec::new(),
    };
    runner
        .run_batch(test_context(1).with_batch("batch:test", tasks.len()), batch)
        .unwrap()
}
