use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures::{SinkExt, StreamExt};
use mutsuki_web_bridge::{HandleOutcome, WebBridge};
use mutsuki_web_protocol::{WebShellAssets, WireMessage};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::WebHostConfig;
use crate::error::{WebHostError, WebHostResult};

#[derive(Clone)]
struct AppState {
    bridge: WebBridge,
    shell_root: PathBuf,
    index_file: String,
    connections: Arc<AtomicU64>,
    budgets_max_connections: usize,
}

pub struct HostServer {
    config: WebHostConfig,
    bridge: WebBridge,
    shell: WebShellAssets,
}

impl HostServer {
    pub fn new(
        config: WebHostConfig,
        bridge: WebBridge,
        shell: WebShellAssets,
        _cancel: CancellationToken,
    ) -> Self {
        Self {
            config,
            bridge,
            shell,
        }
    }

    pub async fn serve(
        self,
        ready_tx: oneshot::Sender<WebHostResult<SocketAddr>>,
        stop_rx: oneshot::Receiver<()>,
        cancel: CancellationToken,
    ) -> WebHostResult<()> {
        let connections = Arc::new(AtomicU64::new(0));
        let state = AppState {
            bridge: self.bridge.clone(),
            shell_root: self.shell.root_dir.clone(),
            index_file: self.shell.index_file.clone(),
            connections: connections.clone(),
            budgets_max_connections: self.config.budgets.max_connections,
        };

        let app = Router::new()
            .route("/health", get(health_handler))
            .route("/ws", get(ws_handler))
            .route("/", get(index_handler))
            .route("/{*path}", get(static_handler))
            .with_state(state);

        // Axum/Hyper types stay inside this module and are not part of the stable ABI.
        let addr = self
            .config
            .listen
            .socket_addr()
            .parse::<SocketAddr>()
            .map_err(|err| WebHostError::InvalidConfig(err.to_string()))?;
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|err| WebHostError::StartFailed(err.to_string()))?;
        let local = listener
            .local_addr()
            .map_err(|err| WebHostError::StartFailed(err.to_string()))?;
        let _ = ready_tx.send(Ok(local));

        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = stop_rx => {}
            }
        });

        server
            .await
            .map_err(|err| WebHostError::StopFailed(err.to_string()))?;
        Ok(())
    }
}

async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let metrics = state.bridge.metrics();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "healthy": true,
            "active_sessions": state.bridge.active_sessions(),
            "safe_mode": state.bridge.safe_mode(),
            "metrics": metrics,
        })
        .to_string(),
    )
}

async fn index_handler(State(state): State<AppState>) -> Response {
    serve_file(&state.shell_root.join(&state.index_file), true).await
}

async fn static_handler(
    State(state): State<AppState>,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response {
    // Prevent path traversal.
    if path.contains("..") {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }
    let file_path = state.shell_root.join(&path);
    serve_file(&file_path, false).await
}

async fn serve_file(path: &std::path::Path, is_index: bool) -> Response {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let mime = mime_guess::from_path(path)
                .first_or_octet_stream()
                .essence_str()
                .to_string();
            let mut response = (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime),
                    (
                        header::CONTENT_SECURITY_POLICY,
                        "default-src 'self'; connect-src 'self' ws: wss:; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'self'".into(),
                    ),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
                ],
                bytes,
            )
                .into_response();
            if !is_index {
                // Only content-addressed names may be immutable; stable names revalidate.
                let cache = if is_content_addressed_asset(path) {
                    HeaderValue::from_static("public, max-age=31536000, immutable")
                } else {
                    HeaderValue::from_static("no-cache")
                };
                response.headers_mut().insert(header::CACHE_CONTROL, cache);
            }
            response
        }
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// True when the file stem ends with `.<hex≥8>` (e.g. `app.a1b2c3d4e5f6.js`).
fn is_content_addressed_asset(path: &std::path::Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    let Some((_, hash)) = stem.rsplit_once('.') else {
        return false;
    };
    hash.len() >= 8 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let current = state.connections.load(Ordering::Relaxed) as usize;
    if current >= state.budgets_max_connections {
        return (StatusCode::TOO_MANY_REQUESTS, "connection budget exceeded").into_response();
    }
    state.connections.fetch_add(1, Ordering::Relaxed);
    state
        .bridge
        .set_connections(state.connections.load(Ordering::Relaxed));
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut session_id: Option<Uuid> = None;
    let mut event_ready: Option<Arc<tokio::sync::Notify>> = None;

    loop {
        let message = if let Some(notifier) = event_ready.as_ref() {
            tokio::select! {
                message = receiver.next() => message,
                _ = notifier.notified() => {
                    let Some(sid) = session_id else { continue };
                    if !send_queued_events(&mut sender, &state, sid).await {
                        break;
                    }
                    continue;
                }
            }
        } else {
            receiver.next().await
        };
        let Some(message) = message else { break };
        let Ok(message) = message else {
            break;
        };
        let bytes = match message {
            Message::Binary(bytes) => bytes,
            Message::Text(_) => {
                let _ = sender
                    .send(binary_error(
                        "unsupported_frame",
                        "web bridge requires binary frames",
                    ))
                    .await;
                break;
            }
            _ => continue,
        };
        if bytes.len() > state.bridge.budgets().max_payload_bytes {
            let _ = sender
                .send(binary_error("payload_too_large", "payload exceeds budget"))
                .await;
            break;
        }

        let parsed = WireMessage::decode(&bytes);
        let Ok(wire) = parsed else {
            let _ = sender
                .send(binary_error("invalid_message", "invalid wire message"))
                .await;
            continue;
        };

        match state.bridge.handle_message_async(session_id, wire).await {
            Ok(HandleOutcome::Reply(reply)) => {
                if let WireMessage::HelloAck { session, .. } = &reply {
                    session_id = Some(session.session_id);
                    event_ready = state.bridge.session_event_notifier(session.session_id);
                }
                if let Ok(bytes) = reply.encode()
                    && sender.send(Message::Binary(bytes.into())).await.is_err()
                {
                    break;
                }
            }
            Ok(HandleOutcome::Subscribed(_)) | Ok(HandleOutcome::Unsubscribed(_)) => {}
            Err(err) => {
                let _ = sender
                    .send(binary_error("bridge_error", &err.to_string()))
                    .await;
            }
        }

        if let Some(sid) = session_id {
            if !send_queued_events(&mut sender, &state, sid).await {
                break;
            }
        }
    }

    if let Some(session_id) = session_id {
        state.bridge.close_session(session_id);
    }
    state.connections.fetch_sub(1, Ordering::Relaxed);
    state
        .bridge
        .set_connections(state.connections.load(Ordering::Relaxed));
}

async fn send_queued_events(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    state: &AppState,
    session_id: Uuid,
) -> bool {
    let events = state.bridge.take_events(session_id);
    state.bridge.set_ws_queue_depth(events.len() as u64);
    for event in events {
        if let Ok(bytes) = WireMessage::Event(event).encode()
            && sender.send(Message::Binary(bytes.into())).await.is_err()
        {
            return false;
        }
    }
    true
}

fn binary_error(code: &str, message: &str) -> Message {
    match (WireMessage::Error {
        code: code.into(),
        message: message.into(),
    })
    .encode()
    {
        Ok(bytes) => Message::Binary(bytes.into()),
        Err(_) => Message::Close(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{binary_error, is_content_addressed_asset};
    use axum::extract::ws::Message;
    use mutsuki_web_protocol::WireMessage;
    use std::path::Path;

    #[test]
    fn detects_content_addressed_filenames() {
        assert!(is_content_addressed_asset(Path::new(
            "assets/app.a1b2c3d4e5f67890.js"
        )));
        assert!(!is_content_addressed_asset(Path::new("mutsuki-ui.css")));
        assert!(!is_content_addressed_asset(Path::new("index.js")));
        assert!(!is_content_addressed_asset(Path::new("foo.bar.js")));
    }

    #[test]
    fn socket_errors_are_binary_wire_messages() {
        let Message::Binary(bytes) = binary_error("invalid_message", "invalid wire message") else {
            panic!("expected binary error");
        };
        assert!(matches!(
            WireMessage::decode(&bytes).expect("decode"),
            WireMessage::Error { code, .. } if code == "invalid_message"
        ));
    }
}
