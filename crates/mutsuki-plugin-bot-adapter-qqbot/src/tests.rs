use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use mutsuki_bot_protocol::{
    BOT_MEDIA_UPLOAD_PROTOCOL_ID, BOT_MESSAGE_RECALL_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID,
    BotEventKind, BotMessage, BotMessageRecallRequest, BotTarget, MessageSegment,
    QQBOT_ACCOUNT_GET_PROTOCOL_ID, QQBOT_GATEWAY_STATUS_PROTOCOL_ID, QQBOT_RAW_CALL_PROTOCOL_ID,
};
use mutsuki_runtime_contracts::{
    BatchEntry, BatchPayload, CompletionBatch, DispatchLane, OrderingRequirement, RunnerResult,
    RunnerSideEffect, RuntimeError, Task, WorkBatch, WorkResourcePlan,
};
use mutsuki_runtime_core::{Runner, RunnerContext};
use serde_json::{Value, json};

use crate::api::{
    HttpMethod, MediaChunk, QqAuthManager, QqMediaError, QqMediaProvider, QqOpenApiError,
    QqOpenApiTransport,
};
use crate::config::QqBotConfig;
use crate::gateway::{GatewayAction, QqGatewayPump};
use crate::tasks::{
    QQBOT_GATEWAY_FRAME_PROTOCOL_ID, QqGatewayMapRunner, QqOpenApiRunner, openapi_descriptor,
    qqbot_adapter_manifest,
};
use crate::{
    QqBotClients, QqHttpClient, QqHttpRequest, QqHttpResponse, QqIdSource, StaticQqCredentials,
};

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
    let event: mutsuki_bot_protocol::BotEvent =
        serde_json::from_value(result.tasks[0].payload.clone()).unwrap();
    assert_eq!(event.kind, BotEventKind::MessageCreated);
    let message = event.message.unwrap();
    assert_eq!(message.plain_text(), "ping");
    assert_eq!(message.time_ms, None);
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
        .map(|entry| {
            serde_json::from_value::<mutsuki_bot_protocol::BotEvent>(
                entry.result.as_ref().unwrap().tasks[0].payload.clone(),
            )
            .unwrap()
        })
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
        .map(|entry| {
            serde_json::from_value::<mutsuki_bot_protocol::BotEvent>(
                entry.result.as_ref().unwrap().tasks[0].payload.clone(),
            )
            .unwrap()
        })
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
    let event: mutsuki_bot_protocol::BotEvent =
        serde_json::from_value(result.tasks[0].payload.clone()).unwrap();

    assert_eq!(
        event.message.unwrap().plain_text(),
        "/echo hello <@OTHER_USER>"
    );
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
    assert_eq!(response["intents"], 1_325_405_185);
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
    assert!(completion.results[0].error.is_some());
    assert!(completion.results[1].error.is_none());
    assert_eq!(
        completion.results[1].result.as_ref().unwrap().events[0].payload["task_id"],
        "account"
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

    pump.handle_raw_frame(json!({"op": 11, "d": null}), 0)
        .unwrap();
    assert_eq!(pump.pop_action(), Some(GatewayAction::AckHeartbeat));
    pump.handle_raw_frame(json!({"op": 7, "d": null}), 0)
        .unwrap();
    assert_eq!(pump.pop_action(), Some(GatewayAction::Reconnect));
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

fn openapi_runner_with_shared(
    requests: Arc<Mutex<Vec<QqHttpRequest>>>,
    responses: Vec<Result<QqHttpResponse, QqOpenApiError>>,
    id_source: Box<dyn QqIdSource>,
) -> QqOpenApiRunner {
    let config = QqBotConfig::new("main", "APP_ID");
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
        _resource_ref: &str,
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
        Some("task-lease-test".into()),
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
            entry_id: format!("entry-{index}"),
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
        batch_key: runner.descriptor().runner_id.clone(),
        entries,
        payload: BatchPayload::from_tasks(&tasks),
        resource_plan: WorkResourcePlan::empty(),
        task_leases: Vec::new(),
    };
    runner
        .run_batch(test_context(1).with_batch("batch:test", tasks.len()), batch)
        .unwrap()
}
