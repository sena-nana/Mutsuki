use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use mutsuki_plugin_bot_adapter_qqbot::QqBotConfig;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

pub mod benchmark;
pub use benchmark::*;

const HEARTBEAT_ACK: &str = r#"{"op":11}"#;

fn gateway_frame_opcode(text: &str) -> Option<u64> {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|frame| frame.get("op").and_then(Value::as_u64))
}

#[derive(Clone, Debug, Default)]
pub struct FakeQqSnapshot {
    pub sends: Vec<Value>,
    pub gateway_auth_frames: Vec<Value>,
    pub websocket_connections: usize,
    pub account_checks: usize,
    pub clean_closes: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FakeQqIdentifyOutcome {
    #[default]
    ReadyThenCleanClose,
    ReadyThenClose(u16),
    ReadyThenReconnectOpcode,
    RejectIdentify,
}

#[derive(Clone, Debug)]
pub struct FakeQqGatewayScript {
    pub initial_events: Vec<Value>,
    pub resumed_events: Vec<Value>,
    pub close_delay: Duration,
    pub after_identify: FakeQqIdentifyOutcome,
}

impl Default for FakeQqGatewayScript {
    fn default() -> Self {
        Self {
            initial_events: vec![command_event(2, "echo-1", "/echo hello")],
            resumed_events: vec![command_event(4, "ping-1", "/ping")],
            close_delay: Duration::ZERO,
            after_identify: FakeQqIdentifyOutcome::ReadyThenCleanClose,
        }
    }
}

/// External-boundary fake for product E2E tests. It runs real HTTP and WebSocket servers while
/// leaving ServiceRuntime, EventSource, Runner routing and task completion untouched.
pub struct FakeQqServer {
    http_addr: std::net::SocketAddr,
    ws_addr: std::net::SocketAddr,
    sends: Arc<Mutex<Vec<Value>>>,
    gateway_auth_frames: Arc<Mutex<Vec<Value>>>,
    websocket_connections: Arc<AtomicUsize>,
    account_checks: Arc<AtomicUsize>,
    clean_closes: Arc<AtomicUsize>,
    send_notify: Arc<Notify>,
    http_task: JoinHandle<()>,
    websocket_task: JoinHandle<()>,
}

impl FakeQqServer {
    pub async fn start() -> Self {
        Self::start_with_gateway_script(FakeQqGatewayScript::default()).await
    }

    pub async fn start_with_gateway_script(script: FakeQqGatewayScript) -> Self {
        let sends = Arc::new(Mutex::new(Vec::new()));
        let gateway_auth_frames = Arc::new(Mutex::new(Vec::new()));
        let websocket_connections = Arc::new(AtomicUsize::new(0));
        let account_checks = Arc::new(AtomicUsize::new(0));
        let clean_closes = Arc::new(AtomicUsize::new(0));
        let send_notify = Arc::new(Notify::new());

        let ws_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_addr = ws_listener.local_addr().unwrap();
        let websocket_task = tokio::spawn(run_gateway_server(
            ws_listener,
            gateway_auth_frames.clone(),
            websocket_connections.clone(),
            clean_closes.clone(),
            script,
        ));

        let http_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_addr = http_listener.local_addr().unwrap();
        let http_task = tokio::spawn(run_http_server(
            http_listener,
            ws_addr,
            sends.clone(),
            send_notify.clone(),
            account_checks.clone(),
        ));

        Self {
            http_addr,
            ws_addr,
            sends,
            gateway_auth_frames,
            websocket_connections,
            account_checks,
            clean_closes,
            send_notify,
            http_task,
            websocket_task,
        }
    }

    pub fn config(&self, account_id: &str, app_id: &str, secret_key: &str) -> QqBotConfig {
        let mut config = QqBotConfig::new(account_id, app_id);
        config.client_secret_key = secret_key.into();
        config.token_url = format!("http://{}/token", self.http_addr);
        config.openapi_base_url = format!("http://{}", self.http_addr);
        config.allow_insecure_transport = true;
        config.gateway_hello_timeout_ms = 1_000;
        config.gateway_ack_timeout_ms = 500;
        config.retry_base_delay_ms = 0;
        config.retry_max_delay_ms = 0;
        config.reconnect_initial_delay_ms = 10;
        config.reconnect_max_delay_ms = 20;
        config.reconnect_jitter_ms = 0;
        config
    }

    pub fn websocket_addr(&self) -> std::net::SocketAddr {
        self.ws_addr
    }

    pub async fn wait_for_sends(&self, count: usize, timeout: Duration) -> Vec<Value> {
        tokio::time::timeout(timeout, async {
            loop {
                let sends = self.sends.lock().unwrap().clone();
                if sends.len() >= count {
                    break sends;
                }
                self.send_notify.notified().await;
            }
        })
        .await
        .expect("fake QQ OpenAPI sends timed out")
    }

    pub async fn wait_for_auth_frames(&self, count: usize, timeout: Duration) -> Vec<Value> {
        tokio::time::timeout(timeout, async {
            loop {
                let frames = self.gateway_auth_frames.lock().unwrap().clone();
                if frames.len() >= count {
                    break frames;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fake QQ Gateway auth frames timed out")
    }

    pub fn snapshot(&self) -> FakeQqSnapshot {
        FakeQqSnapshot {
            sends: self.sends.lock().unwrap().clone(),
            gateway_auth_frames: self.gateway_auth_frames.lock().unwrap().clone(),
            websocket_connections: self.websocket_connections.load(Ordering::SeqCst),
            account_checks: self.account_checks.load(Ordering::SeqCst),
            clean_closes: self.clean_closes.load(Ordering::SeqCst),
        }
    }

    pub async fn shutdown(self) -> FakeQqSnapshot {
        let FakeQqServer {
            sends,
            gateway_auth_frames,
            websocket_connections,
            account_checks,
            clean_closes,
            send_notify: _,
            http_task,
            websocket_task,
            ..
        } = self;
        http_task.abort();
        let _ = http_task.await;
        let _ = tokio::time::timeout(Duration::from_secs(2), websocket_task).await;
        FakeQqSnapshot {
            sends: sends.lock().unwrap().clone(),
            gateway_auth_frames: gateway_auth_frames.lock().unwrap().clone(),
            websocket_connections: websocket_connections.load(Ordering::SeqCst),
            account_checks: account_checks.load(Ordering::SeqCst),
            clean_closes: clean_closes.load(Ordering::SeqCst),
        }
    }
}

async fn run_http_server(
    listener: TcpListener,
    ws_addr: std::net::SocketAddr,
    sends: Arc<Mutex<Vec<Value>>>,
    notify: Arc<Notify>,
    account_checks: Arc<AtomicUsize>,
) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        let sends = sends.clone();
        let notify = notify.clone();
        let account_checks = account_checks.clone();
        tokio::spawn(async move {
            serve_http(stream, ws_addr, sends, notify, account_checks).await;
        });
    }
}

async fn serve_http(
    mut stream: TcpStream,
    ws_addr: std::net::SocketAddr,
    sends: Arc<Mutex<Vec<Value>>>,
    notify: Arc<Notify>,
    account_checks: Arc<AtomicUsize>,
) {
    let (head, body) = read_http_request(&mut stream).await;
    let first_line = head.lines().next().unwrap_or_default();
    let response = if first_line.starts_with("POST /token ") {
        json!({"access_token": "TEST_ACCESS_TOKEN", "expires_in": 7200})
    } else if first_line.starts_with("GET /users/@me ") {
        account_checks.fetch_add(1, Ordering::SeqCst);
        json!({"id": "BOT_OPENID", "username": "fake-bot", "bot": true})
    } else if first_line.starts_with("GET /gateway/bot ") {
        json!({"url": format!("ws://{ws_addr}")})
    } else if let Some(group_openid) = fake_group_info_openid(first_line) {
        json!({
            "group_openid": group_openid,
            "group_name": "测试群",
            "group_member_num": 2
        })
    } else if first_line.starts_with("POST /v2/groups/GROUP_1/messages ") {
        let value: Value = serde_json::from_str(&body).unwrap();
        sends.lock().unwrap().push(value);
        notify.notify_waiters();
        json!({"id": "REPLY"})
    } else {
        json!({"code": 404, "message": format!("unexpected fake request: {first_line}")})
    };
    let bytes = response.to_string();
    let mut response_head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n", bytes.len());
    for (key, value) in BTreeMap::from([
        ("Content-Type", "application/json"),
        ("Connection", "close"),
    ]) {
        response_head.push_str(&format!("{key}: {value}\r\n"));
    }
    response_head.push_str("\r\n");
    stream.write_all(response_head.as_bytes()).await.unwrap();
    stream.write_all(bytes.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
}

fn fake_group_info_openid(first_line: &str) -> Option<String> {
    let rest = first_line.strip_prefix("GET /v2/groups/")?;
    let path = rest.split(' ').next().unwrap_or(rest);
    let group_openid = path.strip_suffix("/info")?;
    (!group_openid.is_empty() && !group_openid.contains('/')).then(|| group_openid.to_owned())
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
    (
        head,
        String::from_utf8(bytes[header_end..header_end + content_length].to_vec()).unwrap(),
    )
}

async fn run_gateway_server(
    listener: TcpListener,
    auth_frames: Arc<Mutex<Vec<Value>>>,
    connections: Arc<AtomicUsize>,
    clean_closes: Arc<AtomicUsize>,
    script: FakeQqGatewayScript,
) {
    let resume_url = format!("ws://{}", listener.local_addr().unwrap());
    for connection in 0..2 {
        let (stream, _) = listener.accept().await.unwrap();
        connections.fetch_add(1, Ordering::SeqCst);
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket
            .send(Message::Text(
                json!({"op": 10, "d": {"heartbeat_interval": 50}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let auth = next_auth_frame(&mut socket).await;
        auth_frames.lock().unwrap().push(auth.clone());
        if connection == 0 {
            match script.after_identify {
                FakeQqIdentifyOutcome::ReadyThenCleanClose => {
                    send_ready_events(&mut socket, &resume_url, &script.initial_events).await;
                    socket.send(Message::Close(None)).await.unwrap();
                }
                FakeQqIdentifyOutcome::ReadyThenClose(code) => {
                    send_ready_events(&mut socket, &resume_url, &script.initial_events).await;
                    socket
                        .send(Message::Close(Some(CloseFrame {
                            code: CloseCode::from(code),
                            reason: format!("close {code}").into(),
                        })))
                        .await
                        .unwrap();
                }
                FakeQqIdentifyOutcome::ReadyThenReconnectOpcode => {
                    send_ready_events(&mut socket, &resume_url, &script.initial_events).await;
                    socket
                        .send(Message::Text(
                            json!({"op": 7, "d": null}).to_string().into(),
                        ))
                        .await
                        .unwrap();
                    drain_until_close(&mut socket, script.close_delay, &clean_closes).await;
                }
                FakeQqIdentifyOutcome::RejectIdentify => {
                    socket
                        .send(Message::Text(
                            json!({"op": 9, "d": false}).to_string().into(),
                        ))
                        .await
                        .unwrap();
                    drain_until_close(&mut socket, script.close_delay, &clean_closes).await;
                }
            }
            continue;
        }
        if auth["op"] == 6 {
            socket
                .send(Message::Text(
                    json!({"op": 0, "s": 3, "t": "RESUMED", "id": "resumed-1", "d": {}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            for event in &script.resumed_events {
                send_gateway_event(&mut socket, event).await;
            }
        } else {
            send_ready_events(&mut socket, &resume_url, &script.initial_events).await;
        }
        drain_until_close(&mut socket, script.close_delay, &clean_closes).await;
    }
}

async fn send_ready_events<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    resume_url: &str,
    events: &[Value],
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    socket
        .send(Message::Text(
            json!({
                "op": 0, "s": 1, "t": "READY", "id": "ready-1",
                "d": {"session_id": "SESSION_1", "resume_gateway_url": resume_url}
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    for event in events {
        send_gateway_event(socket, event).await;
    }
}

async fn drain_until_close<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    close_delay: Duration,
    clean_closes: &Arc<AtomicUsize>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    while let Some(message) = socket.next().await {
        match message {
            Ok(Message::Text(text)) => {
                if gateway_frame_opcode(text.as_ref()) == Some(1) {
                    socket
                        .send(Message::Text(HEARTBEAT_ACK.into()))
                        .await
                        .unwrap();
                }
            }
            Ok(Message::Close(_)) => {
                tokio::time::sleep(close_delay).await;
                clean_closes.fetch_add(1, Ordering::SeqCst);
                break;
            }
            Err(_) => break,
            _ => {}
        }
    }
}

async fn send_gateway_event<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>, event: &Value)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    socket
        .send(Message::Text(event.to_string().into()))
        .await
        .unwrap();
}

fn command_event(sequence: u64, id: &str, content: &str) -> Value {
    json!({
        "op": 0,
        "s": sequence,
        "t": "GROUP_AT_MESSAGE_CREATE",
        "id": id,
        "d": {
            "id": format!("MESSAGE_{sequence}"),
            "group_openid": "GROUP_1",
            "content": format!("<@BOT_OPENID> {content}"),
            "mentions": [{"id": "BOT_OPENID", "is_you": true, "bot": true}],
            "timestamp": "2026-07-12T10:00:00+08:00",
            "author": {"member_openid": "USER_1", "username": "tester"}
        }
    })
}

async fn next_auth_frame<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> Value
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(text.as_ref()).unwrap();
                if value.get("op").and_then(Value::as_u64) == Some(1) {
                    socket
                        .send(Message::Text(HEARTBEAT_ACK.into()))
                        .await
                        .unwrap();
                    continue;
                }
                return value;
            }
            Message::Ping(payload) => {
                socket.send(Message::Pong(payload)).await.unwrap();
            }
            other => panic!("expected Gateway auth frame, received {other:?}"),
        }
    }
}
