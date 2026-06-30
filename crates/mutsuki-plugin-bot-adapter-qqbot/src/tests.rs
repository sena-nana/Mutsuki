use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use mutsuki_bot_protocol::{
    BOT_MESSAGE_RECALL_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID, BotEventKind, BotMessage,
    BotMessageRecallRequest, BotTarget, MessageSegment, QQBOT_ACCOUNT_GET_PROTOCOL_ID,
    QQBOT_GATEWAY_STATUS_PROTOCOL_ID, QQBOT_RAW_CALL_PROTOCOL_ID,
};
use mutsuki_runtime_contracts::Task;
use mutsuki_runtime_core::{Runner, RunnerContext};
use serde_json::{Value, json};

use crate::api::{HttpMethod, MediaChunk, QqMediaError, QqMediaProvider, QqOpenApiError};
use crate::config::QqBotConfig;
use crate::gateway::{GatewayAction, QqGatewayPump};
use crate::tasks::{
    QQBOT_GATEWAY_FRAME_PROTOCOL_ID, QqGatewayMapRunner, QqOpenApiRunner, openapi_descriptor,
};
use crate::{QqBotClients, QqHttpClient, QqHttpRequest, QqHttpResponse, QqIdSource};

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

    let result = runner.step(test_context(1), vec![task]).unwrap();

    assert_eq!(result[0].tasks.len(), 1);
    let event: mutsuki_bot_protocol::BotEvent =
        serde_json::from_value(result[0].tasks[0].payload.clone()).unwrap();
    assert_eq!(event.kind, BotEventKind::MessageCreated);
    let message = event.message.unwrap();
    assert_eq!(message.plain_text(), "ping");
    assert_eq!(message.time_ms, None);
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
            reply_to: None,
            time_ms: None,
            ext: Default::default(),
        })
        .unwrap(),
    );

    let result = runner.step(test_context(1), vec![task]).unwrap();

    assert_eq!(result[0].events[0].payload["response"]["id"], "MESSAGE_ID");
    let requests = requests.lock().unwrap();
    assert_eq!(requests[1].method, HttpMethod::Post);
    assert_eq!(requests[1].headers["Authorization"], "QQBot TOKEN_A");
    assert_eq!(requests[1].body.as_ref().unwrap()["msg_seq"], 700);
    assert_eq!(requests[1].body.as_ref().unwrap()["content"], "hello");
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

    let result = runner.step(test_context(1), vec![task]).unwrap();

    assert_eq!(result[0].events[0].payload["response"]["ok"], true);
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

    let result = runner.step(test_context(1), vec![task]).unwrap();

    let response = &result[0].events[0].payload["response"];
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

    let result = runner.step(test_context(1), vec![task]).unwrap();

    let response = &result[0].events[0].payload["response"];
    assert_eq!(response["account_id"], "main");
    assert_eq!(response["platform"], "qqbot");
    assert_eq!(response["gateway"]["url"], "wss://gateway.example.invalid");
    assert_eq!(response["shard"], json!([0, 1]));
    assert_eq!(response["intents"], 1_325_405_185);
    let requests = requests.lock().unwrap();
    assert_eq!(requests[1].method, HttpMethod::Get);
    assert!(requests[1].url.ends_with("/gateway"));
    assert_eq!(requests[1].body, None);
    assert_eq!(requests[1].headers["Authorization"], "QQBot TOKEN_A");
}

#[test]
fn openapi_descriptor_accepts_manifest_provided_qqbot_protocols() {
    let descriptor = openapi_descriptor(1);

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

    let result = runner.step(test_context(1), vec![task]);

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

    let result = runner.step(test_context(1), vec![task]);

    assert!(result.is_err());
    assert!(requests.lock().unwrap().is_empty());
}

fn openapi_runner_with_shared(
    requests: Arc<Mutex<Vec<QqHttpRequest>>>,
    responses: Vec<Result<QqHttpResponse, QqOpenApiError>>,
    id_source: Box<dyn QqIdSource>,
) -> QqOpenApiRunner {
    let config = QqBotConfig::new("main", "APP_ID", "CLIENT_SECRET");
    let clients = QqBotClients::new(
        Box::new(FakeHttpClient {
            requests,
            responses: Mutex::new(VecDeque::from(responses)),
        }),
        Box::new(FakeMediaProvider),
    );
    QqOpenApiRunner::new(1, config, clients, id_source)
}

fn token_response(token: &str) -> Result<QqHttpResponse, QqOpenApiError> {
    ok_response(json!({"access_token": token, "expires_in": 7200}))
}

fn ok_response(body: Value) -> Result<QqHttpResponse, QqOpenApiError> {
    Ok(QqHttpResponse { status: 200, body })
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
