use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bot_echo::{echo_manifest, echo_runner};
use futures_util::{SinkExt, StreamExt};
use mutsuki_bot_protocol::{BOT_COMMAND_PARSE_PROTOCOL_ID, BotEventKind, BotEventSubscription};
use mutsuki_plugin_bot_adapter_qqbot::{QqBotConfig, QqBotPluginBundle};
use mutsuki_plugin_bot_command::{BotCommandRunner, bot_command_manifest};
use mutsuki_plugin_bot_event_router::{BotEventRouterRunner, bot_event_router_manifest};
use mutsuki_service_config::{IpcTransport, ServiceConfig};
use mutsuki_service_control::{ControlMethod, ControlRequest};
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[tokio::test]
async fn qqbot_bundle_requires_host_secret_during_service_preflight() {
    let secret_key = format!("MISSING_QQBOT_SECRET_{}", std::process::id());
    let mut qqbot_config = QqBotConfig::new("preflight", "TEST_APP_ID");
    qqbot_config.client_secret_key = secret_key.clone();
    let bundle = QqBotPluginBundle::new(qqbot_config).unwrap();
    let builder = bundle
        .install(ServiceRuntimeBuilder::new(ServiceConfig::default()))
        .unwrap();

    let error = match builder.start().await {
        Ok(runtime) => {
            runtime.shutdown().await;
            panic!("QQBot ServiceRuntime unexpectedly started without Host secret")
        }
        Err(error) => error,
    };

    let message = error.to_string();
    assert!(message.contains("requires missing Host secret"));
    assert!(message.contains(&secret_key));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_service_runtime_runs_gateway_resume_echo_and_clean_shutdown() {
    let _env = ENV_LOCK.lock().expect("environment lock");
    let received_send = Arc::new(Mutex::new(None::<Value>));
    let send_notify = Arc::new(Notify::new());
    let gateway_auth_frames = Arc::new(Mutex::new(Vec::<Value>::new()));
    let websocket_connections = Arc::new(Mutex::new(0_u64));
    let account_checks = Arc::new(AtomicUsize::new(0));
    let clean_closes = Arc::new(AtomicUsize::new(0));

    let ws_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_addr = ws_listener.local_addr().unwrap();
    let resume_url = format!("ws://{ws_addr}");
    let ws_auth = gateway_auth_frames.clone();
    let ws_connections = websocket_connections.clone();
    let ws_clean_closes = clean_closes.clone();
    let ws_task = tokio::spawn(async move {
        run_gateway_server(
            ws_listener,
            resume_url,
            ws_auth,
            ws_connections,
            ws_clean_closes,
        )
        .await
    });

    let http_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_addr = http_listener.local_addr().unwrap();
    let http_gateway_url = format!("ws://{ws_addr}");
    let http_send = received_send.clone();
    let http_notify = send_notify.clone();
    let http_account_checks = account_checks.clone();
    let http_task = tokio::spawn(async move {
        loop {
            let (stream, _) = http_listener.accept().await.unwrap();
            let gateway_url = http_gateway_url.clone();
            let send = http_send.clone();
            let notify = http_notify.clone();
            let account_checks = http_account_checks.clone();
            tokio::spawn(async move {
                serve_http(stream, &gateway_url, send, notify, account_checks).await;
            });
        }
    });

    let secret_key = format!("QQBOT_TEST_SECRET_{}", http_addr.port());
    let secret_env = format!("MUTSUKI_SECRET_{secret_key}");
    unsafe { std::env::set_var(&secret_env, "TEST_CLIENT_SECRET") };

    let dir = tempfile::tempdir().unwrap();
    let ipc_probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ipc_addr = ipc_probe.local_addr().unwrap();
    drop(ipc_probe);
    let mut service_config = ServiceConfig::default();
    service_config.service.instance_id = "qqbot-integration".into();
    service_config.service.profile = "qqbot-test".into();
    service_config.ipc.enabled = true;
    service_config.ipc.transport = IpcTransport::TcpDebug;
    service_config.ipc.tcp_debug_addr = Some(ipc_addr.to_string());
    service_config.ipc.token = Some("test-token".into());
    service_config.observe.console = false;
    service_config.service.home_dir = dir.path().to_path_buf();
    service_config.service.log_dir = dir.path().join("logs");
    service_config.service.run_dir = dir.path().join("run");
    service_config.plugins.builtin.clear();
    service_config.plugins.dynamic_dirs.clear();
    service_config.plugins.disabled_dir = dir.path().join("disabled");
    service_config.runners.graceful_shutdown_ms = 1_000;
    std::fs::create_dir_all(&service_config.service.log_dir).unwrap();
    std::fs::create_dir_all(&service_config.service.run_dir).unwrap();

    let mut qqbot_config = QqBotConfig::new("integration", "TEST_APP_ID");
    qqbot_config.client_secret_key = secret_key;
    qqbot_config.token_url = format!("http://{http_addr}/token");
    qqbot_config.openapi_base_url = format!("http://{http_addr}");
    qqbot_config.allow_insecure_transport = true;
    qqbot_config.gateway_hello_timeout_ms = 1_000;
    qqbot_config.gateway_ack_timeout_ms = 500;
    qqbot_config.retry_base_delay_ms = 0;
    qqbot_config.retry_max_delay_ms = 0;
    qqbot_config.reconnect_initial_delay_ms = 10;
    qqbot_config.reconnect_max_delay_ms = 20;
    qqbot_config.reconnect_jitter_ms = 0;

    let control_config = service_config.clone();
    let subscriptions = vec![BotEventSubscription {
        subscription_id: "qqbot-message-to-command".into(),
        handler_protocol_id: BOT_COMMAND_PARSE_PROTOCOL_ID.into(),
        handler_binding_id: None,
        platform: Some("qqbot".into()),
        event_kind: Some(BotEventKind::MessageCreated),
    }];
    let builder = ServiceRuntimeBuilder::new(service_config)
        .register_builtin_plugin(bot_event_router_manifest(1))
        .register_builtin_plugin(bot_command_manifest(1))
        .register_builtin_plugin(echo_manifest(1))
        .register_builtin_runner(move || {
            Box::new(BotEventRouterRunner::new(1, subscriptions.clone()))
        })
        .register_builtin_runner(|| Box::new(BotCommandRunner::new(1, vec!["/".into()])))
        .register_builtin_runner(|| echo_runner(1));
    let bundle = QqBotPluginBundle::new(qqbot_config).unwrap();
    let health = bundle.health_handle();
    let builder = bundle.install(builder).unwrap();

    let runtime = builder.start().await.unwrap();

    tokio::time::timeout(Duration::from_secs(5), send_notify.notified())
        .await
        .expect("echo OpenAPI send observed");
    let body = received_send
        .lock()
        .unwrap()
        .clone()
        .expect("captured send body");
    assert_eq!(body["content"], "hello");
    assert_eq!(body["msg_type"], 0);
    assert_eq!(body["msg_id"], "MESSAGE_1");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = health.snapshot();
            if snapshot.identified
                && snapshot.connected
                && snapshot.last_event_unix_ms.is_some()
                && snapshot.last_heartbeat_unix_ms.is_some()
                && snapshot.last_ack_unix_ms.is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("health becomes identified");
    assert_eq!(health.snapshot().reconnect_count, 1);

    let status = control(&control_config, ControlMethod::ServiceStatus).await;
    assert_eq!(status["instance_id"], "qqbot-integration");
    assert_eq!(status["core_running"], true);
    let plugins = control(&control_config, ControlMethod::PluginList).await;
    let plugin_ids = plugins
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|plugin| plugin["plugin_id"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        plugin_ids,
        BTreeSet::from([
            "mutsuki.bot.adapter.qqbot",
            "mutsuki.bot.router.event",
            "mutsuki.bot.command",
            "example.bot.echo",
        ])
    );
    let sources = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let sources = control(&control_config, ControlMethod::EventSourceList).await;
            if sources[0]["health"] == "healthy" {
                break sources;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("ServiceHost observes healthy Gateway source");
    assert_eq!(sources.as_array().unwrap().len(), 1);
    assert_eq!(sources[0]["plugin_id"], "mutsuki.bot.adapter.qqbot");
    assert_eq!(sources[0]["health"], "healthy");
    let service_health = control(&control_config, ControlMethod::HealthCheck).await;
    assert_eq!(service_health["service"], "ok");
    assert_eq!(service_health["core"], "ok");
    assert_eq!(service_health["plugins"], "ok");
    assert_eq!(service_health["event_sources"], "ok");
    let gateway_health = &service_health["components"]["mutsuki.bot.qqbot.gateway:integration"];
    assert_eq!(gateway_health["status"], "ok");
    assert_eq!(gateway_health["connected"], true);
    assert_eq!(gateway_health["identified"], true);
    assert!(gateway_health["last_heartbeat_unix_ms"].is_number());
    assert!(gateway_health["last_ack_unix_ms"].is_number());
    assert!(gateway_health["last_event_unix_ms"].is_number());
    assert_eq!(gateway_health["reconnect_count"], 1);
    assert!(gateway_health["last_error"].is_null());

    let tasks = control(&control_config, ControlMethod::TaskList).await;
    let gateway_task_ids = tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|task| task["protocol_id"] == "mutsuki.bot.qqbot.gateway/frame@1")
        .filter_map(|task| task["task_id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(gateway_task_ids.len(), 3);
    assert!(
        gateway_task_ids
            .iter()
            .all(|task_id| task_id.starts_with("mutsuki.bot.qqbot.gateway.frame:integration:"))
    );
    let task_control_json = tasks.to_string();
    assert!(!task_control_json.contains("TEST_CLIENT_SECRET"));
    assert!(!task_control_json.contains("TEST_ACCESS_TOKEN"));
    let correlated_protocols = tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|task| task["correlation_id"] == "group-event-1")
        .filter_map(|task| task["protocol_id"].as_str())
        .collect::<Vec<_>>();
    assert_pipeline_order(&correlated_protocols);

    control(&control_config, ControlMethod::ServiceShutdown).await;
    runtime
        .run_until_shutdown_signal(std::future::pending::<String>())
        .await
        .unwrap();
    let stopped_health = health.snapshot();
    assert!(!stopped_health.connected);
    assert!(!stopped_health.identified);
    assert_logs_do_not_contain_secrets(&control_config.service.log_dir);
    assert!(TcpStream::connect(ipc_addr).await.is_err());
    tokio::time::timeout(Duration::from_secs(2), ws_task)
        .await
        .expect("Gateway server sees clean socket shutdown")
        .unwrap();
    http_task.abort();
    let _ = http_task.await;
    unsafe { std::env::remove_var(&secret_env) };

    assert_eq!(*websocket_connections.lock().unwrap(), 2);
    assert_eq!(clean_closes.load(Ordering::SeqCst), 2);
    assert_eq!(account_checks.load(Ordering::SeqCst), 2);
    let auth = gateway_auth_frames.lock().unwrap();
    assert_eq!(auth.len(), 3);
    assert_eq!(auth[0]["op"], 2);
    assert_eq!(auth[0]["d"]["intents"], 1_325_405_185_u64);
    assert_eq!(auth[0]["d"]["shard"], json!([0, 1]));
    assert_eq!(auth[0]["d"]["token"], "QQBot TEST_ACCESS_TOKEN");
    assert_eq!(auth[1]["op"], 6);
    assert_eq!(auth[1]["d"]["token"], "QQBot TEST_ACCESS_TOKEN");
    assert_eq!(auth[1]["d"]["session_id"], "SESSION_1");
    assert_eq!(auth[1]["d"]["seq"], 1);
    assert_eq!(auth[2]["op"], 2);
    assert_eq!(auth[2]["d"]["token"], "QQBot TEST_ACCESS_TOKEN");
}

fn assert_logs_do_not_contain_secrets(log_dir: &std::path::Path) {
    for entry in std::fs::read_dir(log_dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        assert!(!text.contains("TEST_CLIENT_SECRET"), "secret in {path:?}");
        assert!(!text.contains("TEST_ACCESS_TOKEN"), "token in {path:?}");
    }
}

async fn control(config: &ServiceConfig, method: ControlMethod) -> Value {
    let response = mutsuki_service_ipc::request(
        config,
        ControlRequest {
            token: "test-token".into(),
            method,
            params: Value::Null,
        },
    )
    .await
    .unwrap();
    assert!(response.ok, "control error: {:?}", response.error);
    response.result.unwrap_or(Value::Null)
}

fn assert_pipeline_order(protocols: &[&str]) {
    let expected = protocols_for_pipeline();
    let mut cursor = 0;
    for protocol in protocols {
        if cursor < expected.len() && *protocol == expected[cursor] {
            cursor += 1;
        }
    }
    assert_eq!(cursor, expected.len(), "pipeline protocols: {protocols:?}");
}

fn protocols_for_pipeline() -> [&'static str; 5] {
    [
        "mutsuki.bot.qqbot.gateway/frame@1",
        "mutsuki.bot.event/ingest@1",
        "mutsuki.bot.command/parse@1",
        "mutsuki.bot.command/handle@1",
        "mutsuki.bot.message/send@1",
    ]
}

async fn run_gateway_server(
    listener: TcpListener,
    resume_url: String,
    auth_frames: Arc<Mutex<Vec<Value>>>,
    connections: Arc<Mutex<u64>>,
    clean_closes: Arc<AtomicUsize>,
) {
    for connection_index in 0..2 {
        let (stream, _) = listener.accept().await.unwrap();
        *connections.lock().unwrap() += 1;
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket
            .send(Message::Text(
                json!({"op": 10, "d": {"heartbeat_interval": 50}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let auth = next_json(&mut socket).await;
        auth_frames.lock().unwrap().push(auth);
        if connection_index == 0 {
            socket
                .send(Message::Text(
                    json!({
                        "op": 0,
                        "s": 1,
                        "t": "READY",
                        "id": "ready-1",
                        "d": {
                            "session_id": "SESSION_1",
                            "resume_gateway_url": resume_url
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            while let Some(message) = socket.next().await {
                match message {
                    Ok(Message::Close(_)) => {
                        clean_closes.fetch_add(1, Ordering::SeqCst);
                        break;
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
        } else {
            socket
                .send(Message::Text(
                    json!({"op": 9, "d": false}).to_string().into(),
                ))
                .await
                .unwrap();
            let reidentify = next_json(&mut socket).await;
            auth_frames.lock().unwrap().push(reidentify);
            socket
                .send(Message::Text(
                    json!({
                        "op": 0,
                        "s": 2,
                        "t": "READY",
                        "id": "ready-2",
                        "d": {
                            "session_id": "SESSION_2",
                            "resume_gateway_url": resume_url
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "op": 0,
                        "s": 3,
                        "t": "GROUP_AT_MESSAGE_CREATE",
                        "id": "group-event-1",
                        "d": {
                            "id": "MESSAGE_1",
                            "group_openid": "GROUP_1",
                            "content": "<@BOT_OPENID> /echo hello",
                            "mentions": [
                                {"id": "BOT_OPENID", "is_you": true, "bot": true}
                            ],
                            "timestamp": "2026-07-11T10:00:00+08:00",
                            "author": {"member_openid": "USER_1", "username": "tester"}
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            while let Some(message) = socket.next().await {
                let Ok(message) = message else {
                    break;
                };
                match message {
                    Message::Text(text) => {
                        let frame: Value = serde_json::from_str(text.as_ref()).unwrap();
                        if frame["op"] == 1 {
                            socket
                                .send(Message::Text(json!({"op": 11}).to_string().into()))
                                .await
                                .unwrap();
                        }
                    }
                    Message::Close(_) => {
                        clean_closes.fetch_add(1, Ordering::SeqCst);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn next_json<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> Value
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
            return serde_json::from_str(text.as_ref()).unwrap();
        }
    }
}

async fn serve_http(
    mut stream: TcpStream,
    gateway_url: &str,
    received_send: Arc<Mutex<Option<Value>>>,
    notify: Arc<Notify>,
    account_checks: Arc<AtomicUsize>,
) {
    let (head, body) = read_http_request(&mut stream).await;
    let first_line = head.lines().next().unwrap_or_default();
    let response = if first_line.starts_with("POST /token ") {
        assert!(body.contains("TEST_CLIENT_SECRET"));
        json!({"access_token": "TEST_ACCESS_TOKEN", "expires_in": 7200})
    } else if first_line.starts_with("GET /users/@me ") {
        account_checks.fetch_add(1, Ordering::SeqCst);
        assert!(
            head.contains("Authorization: QQBot TEST_ACCESS_TOKEN")
                || head.contains("authorization: QQBot TEST_ACCESS_TOKEN")
        );
        json!({"id": "BOT_OPENID", "username": "integration-bot", "bot": true})
    } else if first_line.starts_with("GET /gateway/bot ") {
        assert!(
            head.contains("Authorization: QQBot TEST_ACCESS_TOKEN")
                || head.contains("authorization: QQBot TEST_ACCESS_TOKEN")
        );
        json!({"url": gateway_url})
    } else if first_line.starts_with("POST /v2/groups/GROUP_1/messages ") {
        let value: Value = serde_json::from_str(&body).unwrap();
        *received_send.lock().unwrap() = Some(value);
        notify.notify_one();
        json!({"id": "REPLY_1"})
    } else {
        panic!("unexpected fake QQBot request: {first_line}");
    };
    let bytes = response.to_string();
    let headers = BTreeMap::from([
        ("Content-Type", "application/json"),
        ("Connection", "close"),
    ]);
    let mut response_head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n", bytes.len());
    for (key, value) in headers {
        response_head.push_str(&format!("{key}: {value}\r\n"));
    }
    response_head.push_str("\r\n");
    stream.write_all(response_head.as_bytes()).await.unwrap();
    stream.write_all(bytes.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
}

async fn read_http_request(stream: &mut TcpStream) -> (String, String) {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "HTTP request ended before headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let head = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let content_length = head
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() - header_end < content_length {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "HTTP request ended before body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8(bytes[header_end..header_end + content_length].to_vec()).unwrap();
    (head, body)
}
