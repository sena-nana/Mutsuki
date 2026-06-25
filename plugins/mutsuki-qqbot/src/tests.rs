use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use mutsuki_runtime_contracts::{RunnerPurity, Task};
use mutsuki_runtime_core::{Runner, RunnerContext};
use serde_json::{Value, json};

use crate::config::QqBotConfig;
use crate::gateway::{GatewayAction, GatewayFrame, QqGatewayPump, normalize_gateway_frame};
use crate::manifest::{
    EFFECT_INTERACTION_ACK, EFFECT_MEDIA_UPLOAD, EFFECT_MESSAGE_RECALL, EFFECT_MESSAGE_SEND,
    EFFECT_RUNNER_ID, EFFECT_USER_SHARE_LINK, GATEWAY_NORMALIZER_RUNNER_ID, PLUGIN_ID,
    RAW_GATEWAY_TASK_KIND, qqbot_manifest,
};
use crate::media::{MediaChunk, QqMediaError, QqMediaProvider};
use crate::openapi::{
    QqBotClients, QqHttpClient, QqHttpRequest, QqHttpResponse, QqIdSource, QqOpenApiError,
};
use crate::runner::{QqGatewayNormalizeRunner, QqOpenApiRunner};

#[test]
fn manifest_declares_qqbot_runtime_surfaces() {
    let manifest = qqbot_manifest();

    assert_eq!(manifest.plugin_id, PLUGIN_ID);
    assert_eq!(manifest.provides.runners.len(), 2);
    assert!(manifest.provides.runners.iter().any(|runner| {
        runner.runner_id == GATEWAY_NORMALIZER_RUNNER_ID
            && runner.purity == RunnerPurity::Pure
            && runner.accepted_task_kinds == vec![RAW_GATEWAY_TASK_KIND]
    }));
    assert!(manifest.provides.runners.iter().any(|runner| {
        runner.runner_id == EFFECT_RUNNER_ID
            && runner.purity == RunnerPurity::Effectful
            && runner
                .accepted_task_kinds
                .contains(&EFFECT_MESSAGE_SEND.into())
    }));
    for effect in [
        EFFECT_MESSAGE_SEND,
        EFFECT_MEDIA_UPLOAD,
        EFFECT_MESSAGE_RECALL,
        EFFECT_INTERACTION_ACK,
        EFFECT_USER_SHARE_LINK,
    ] {
        assert!(manifest.provides.effects.contains(&effect.into()));
    }
    assert!(manifest.provides.streams.contains(&"qqbot.gateway".into()));
    assert!(
        manifest
            .provides
            .subscriptions
            .contains(&"qqbot.gateway.events".into())
    );
    assert!(
        manifest
            .provides
            .timers
            .contains(&"qqbot.gateway.heartbeat".into())
    );
}

#[test]
fn gateway_normalizer_maps_documented_events() {
    let cases = [
        ("READY", "qqbot.gateway.ready"),
        ("RESUMED", "qqbot.gateway.resumed"),
        ("GROUP_MESSAGE_CREATE", "qqbot.message.group"),
        ("GROUP_AT_MESSAGE_CREATE", "qqbot.message.group"),
        ("C2C_MESSAGE_CREATE", "qqbot.message.c2c"),
        ("INTERACTION_CREATE", "qqbot.interaction"),
        ("MESSAGE_REACTION_ADD", "qqbot.reaction"),
        ("GROUP_MEMBER_ADD", "qqbot.lifecycle"),
        ("UNEXPECTED", "qqbot.gateway.unknown"),
    ];
    for (event_type, expected_kind) in cases {
        let events = normalize_gateway_frame(GatewayFrame {
            op: 0,
            s: Some(7),
            t: Some(event_type.into()),
            id: Some(format!("{event_type}:id")),
            d: json!({"id": "message-id"}),
        })
        .unwrap();
        assert_eq!(events[0].kind, expected_kind);
        assert_eq!(events[0].payload["dedup_key"], "message:message-id");
    }
}

#[test]
fn gateway_pump_creates_discrete_tasks_and_deduplicates() {
    let mut pump = QqGatewayPump::new();
    let frame = json!({
        "op": 0,
        "s": 23,
        "t": "GROUP_MESSAGE_CREATE",
        "id": "GROUP_MESSAGE_CREATE:event",
        "d": {"id": "message-id", "content": "hi"}
    });

    let task = pump.handle_raw_frame(frame.clone(), 9).unwrap().unwrap();
    assert_eq!(task.kind, RAW_GATEWAY_TASK_KIND);
    assert_eq!(task.registry_generation, 9);
    assert!(matches!(
        pump.pop_action(),
        Some(GatewayAction::DispatchTask(_))
    ));
    assert!(pump.handle_raw_frame(frame, 9).unwrap().is_none());
    assert_eq!(pump.last_sequence(), Some(23));
}

#[test]
fn gateway_pump_tracks_session_and_reconnect_actions() {
    let mut pump = QqGatewayPump::new();
    let ready = json!({
        "op": 0,
        "s": 1,
        "t": "READY",
        "id": "READY:event",
        "d": {"session_id": "SESSION_ID"}
    });

    pump.handle_raw_frame(ready, 1).unwrap();
    assert_eq!(pump.session_id(), Some("SESSION_ID"));
    pump.handle_raw_frame(json!({"op": 7}), 1).unwrap();
    assert_eq!(
        pump.pop_action(),
        Some(GatewayAction::DispatchTask("qqbot.gateway.frame:1".into()))
    );
    assert_eq!(pump.pop_action(), Some(GatewayAction::Reconnect));
}

#[test]
fn gateway_pump_builds_session_control_frames() {
    let mut pump = QqGatewayPump::new();
    let mut config = QqBotConfig::new("APP_ID", "CLIENT_SECRET");
    config.gateway_intents = 123;
    config.shard = [1, 4];
    let identify = QqGatewayPump::identify_frame(&config, "TOKEN");
    assert_eq!(identify["op"], 2);
    assert_eq!(identify["d"]["token"], "QQBot TOKEN");
    assert_eq!(identify["d"]["intents"], 123);
    assert_eq!(identify["d"]["shard"], json!([1, 4]));

    pump.handle_raw_frame(
        json!({
            "op": 0,
            "s": 99,
            "t": "READY",
            "id": "READY:event",
            "d": {"session_id": "SESSION_ID"}
        }),
        1,
    )
    .unwrap();
    let resume = pump.resume_frame("TOKEN").unwrap();
    assert_eq!(resume["op"], 6);
    assert_eq!(resume["d"]["session_id"], "SESSION_ID");
    assert_eq!(resume["d"]["seq"], 99);
    assert_eq!(pump.heartbeat_frame(), json!({"op": 1, "d": 99}));
}

#[test]
fn gateway_runner_emits_normalized_domain_events() {
    let mut runner = QqGatewayNormalizeRunner::new(1);
    let mut task = Task::new(
        "gateway-task",
        RAW_GATEWAY_TASK_KIND,
        json!({
            "op": 0,
            "s": 24,
            "t": "C2C_MESSAGE_CREATE",
            "id": "C2C_MESSAGE_CREATE:event",
            "d": {"id": "message-id", "content": "hi"}
        }),
    );
    task.registry_generation = 1;

    let result = runner
        .step(
            RunnerContext {
                registry_generation: 1,
                current_step: 1,
            },
            vec![task],
        )
        .unwrap();

    assert_eq!(result[0].events[0].kind, "qqbot.message.c2c");
}

#[test]
fn group_message_rejects_c2c_only_fields() {
    let mut runner = openapi_runner_with(
        vec![
            token_response("TOKEN_A"),
            ok_response(json!({"id": "should-not-be-used"})),
        ],
        Box::new(NoopIdSource::new(100)),
    );
    let task = Task::new(
        "send",
        EFFECT_MESSAGE_SEND,
        json!({
            "scene": "group",
            "target_openid": "GROUP_OPENID",
            "body": {
                "msg_type": 2,
                "markdown": {"content": "hello"},
                "stream": {"state": 1, "index": 0}
            }
        }),
    );

    let error = runner
        .step(
            RunnerContext {
                registry_generation: 1,
                current_step: 1,
            },
            vec![task],
        )
        .unwrap_err();

    assert!(
        error
            .error()
            .evidence
            .get("message")
            .is_some_and(|value| format!("{value:?}").contains("stream"))
    );
}

#[test]
fn openapi_runner_refreshes_token_once_after_401() {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let mut runner = openapi_runner_with_shared(
        requests.clone(),
        vec![
            token_response("TOKEN_A"),
            response(401, json!({"message": "expired", "access_token": "SECRET"})),
            token_response("TOKEN_B"),
            ok_response(json!({"id": "MESSAGE_ID"})),
        ],
        Box::new(NoopIdSource::new(700)),
    );
    let task = Task::new(
        "send",
        EFFECT_MESSAGE_SEND,
        json!({
            "scene": "c2c",
            "target_openid": "USER_OPENID",
            "body": {"msg_type": 0, "content": "hello"}
        }),
    );

    let result = runner
        .step(
            RunnerContext {
                registry_generation: 1,
                current_step: 1,
            },
            vec![task],
        )
        .unwrap();

    assert_eq!(result[0].events[0].payload["response"]["id"], "MESSAGE_ID");
    let requests = requests.borrow();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[1].headers["Authorization"], "QQBot TOKEN_A");
    assert_eq!(requests[3].headers["Authorization"], "QQBot TOKEN_B");
    assert_eq!(requests[3].body.as_ref().unwrap()["msg_seq"], 700);
}

#[test]
fn media_upload_fails_when_file_info_is_empty() {
    let mut runner = openapi_runner_with(
        vec![
            token_response("TOKEN_A"),
            ok_response(json!({"file_info": ""})),
        ],
        Box::new(NoopIdSource::new(1)),
    );
    let task = Task::new(
        "media",
        EFFECT_MEDIA_UPLOAD,
        json!({
            "scene": "group",
            "target_openid": "GROUP_OPENID",
            "file_type": 1,
            "url": "https://example.com/image.png"
        }),
    );

    let error = runner
        .step(
            RunnerContext {
                registry_generation: 1,
                current_step: 1,
            },
            vec![task],
        )
        .unwrap_err();

    assert!(
        error
            .error()
            .evidence
            .get("message")
            .is_some_and(|value| format!("{value:?}").contains("file_info"))
    );
}

fn openapi_runner_with(
    responses: Vec<Result<QqHttpResponse, QqOpenApiError>>,
    id_source: Box<dyn QqIdSource>,
) -> QqOpenApiRunner {
    openapi_runner_with_shared(Rc::new(RefCell::new(Vec::new())), responses, id_source)
}

fn openapi_runner_with_shared(
    requests: Rc<RefCell<Vec<QqHttpRequest>>>,
    responses: Vec<Result<QqHttpResponse, QqOpenApiError>>,
    id_source: Box<dyn QqIdSource>,
) -> QqOpenApiRunner {
    let config = QqBotConfig::new("APP_ID", "CLIENT_SECRET");
    let clients = QqBotClients::new(
        Box::new(FakeHttpClient {
            requests,
            responses: RefCell::new(VecDeque::from(responses)),
        }),
        Box::new(FakeMediaProvider),
    );
    QqOpenApiRunner::new(1, config, clients, id_source)
}

fn token_response(token: &str) -> Result<QqHttpResponse, QqOpenApiError> {
    ok_response(json!({"access_token": token, "expires_in": 7200}))
}

fn ok_response(body: Value) -> Result<QqHttpResponse, QqOpenApiError> {
    response(200, body)
}

fn response(status: u16, body: Value) -> Result<QqHttpResponse, QqOpenApiError> {
    Ok(QqHttpResponse { status, body })
}

struct FakeHttpClient {
    requests: Rc<RefCell<Vec<QqHttpRequest>>>,
    responses: RefCell<VecDeque<Result<QqHttpResponse, QqOpenApiError>>>,
}

impl QqHttpClient for FakeHttpClient {
    fn send(&mut self, request: QqHttpRequest) -> Result<QqHttpResponse, QqOpenApiError> {
        self.requests.borrow_mut().push(request);
        self.responses
            .borrow_mut()
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
        Ok(vec![MediaChunk {
            index: 1,
            bytes: vec![1, 2, 3],
            md5: "md5".into(),
        }])
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
