//! Functional tests for Issue #1 acceptance scenarios.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::default_trait_access)]

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use mutsuki_web_extension::content_hash;
use mutsuki_web_extension::{
    EventRegistry, ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor,
};
use mutsuki_web_host::{MinimalWebApplication, MutsukiWebHost, WebHost, WebHostError};
use mutsuki_web_protocol::{
    DeploymentMode, EXTENSION_MANIFEST_VERSION, ExtensionManifest, JsonValue, WEB_PROTOCOL_VERSION,
    WebApplicationDescriptor, WebFrontendAssets, WebHostStatus, WebShellAssets,
};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

struct ExampleExtension {
    manifest: ExtensionManifest,
    assets: Option<WebFrontendAssets>,
    fail: bool,
    extra_img_src: Vec<String>,
}

impl ExampleExtension {
    fn ok(root: &std::path::Path) -> Self {
        let entry_bytes = b"export default { id: 'mutsuki.example.web' }";
        std::fs::write(root.join("index.js"), entry_bytes).unwrap();
        let hash = content_hash(entry_bytes);
        let manifest = ExtensionManifest {
            manifest_version: EXTENSION_MANIFEST_VERSION,
            id: "mutsuki.example.web".into(),
            version: "0.1.0".into(),
            entry: "index.js".into(),
            capabilities: vec!["example.read".into()],
            permissions: vec!["pages".into(), "navigation".into()],
            assets: vec![mutsuki_web_protocol::AssetEntry {
                path: "index.js".into(),
                content_hash: hash,
                bytes: entry_bytes.len() as u64,
            }],
            protocol_version: WEB_PROTOCOL_VERSION.into(),
        };
        std::fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        Self {
            manifest: manifest.clone(),
            assets: Some(WebFrontendAssets {
                manifest,
                root_dir: root.to_path_buf(),
            }),
            fail: false,
            extra_img_src: Vec::new(),
        }
    }

    fn failing() -> Self {
        Self {
            manifest: ExtensionManifest {
                manifest_version: EXTENSION_MANIFEST_VERSION,
                id: "mutsuki.failing.web".into(),
                version: "0.1.0".into(),
                entry: "missing.js".into(),
                capabilities: vec![],
                permissions: vec![],
                assets: vec![],
                protocol_version: WEB_PROTOCOL_VERSION.into(),
            },
            assets: None,
            fail: true,
            extra_img_src: Vec::new(),
        }
    }
}

impl WebExtension for ExampleExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        self.manifest.clone()
    }

    fn frontend_assets(&self) -> Option<WebFrontendAssets> {
        self.assets.clone()
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        if self.fail {
            return Err(ExtensionError::Setup("boom".into()));
        }
        ctx.register("ping", |_params| Ok(JsonValue::String("pong".into())));
        ctx.register_async("ping_async", |_params| async move {
            tokio::task::yield_now().await;
            Ok(JsonValue::String("pong-async".into()))
        });
        Ok(())
    }

    fn register_events(&self, ctx: &mut EventRegistry) -> Result<(), ExtensionError> {
        ctx.register_topic("ticks");
        Ok(())
    }

    fn extra_img_src(&self) -> Vec<String> {
        self.extra_img_src.clone()
    }
}

async fn start_host(safe_mode: bool, extension: Option<ExampleExtension>) -> MutsukiWebHost {
    let shell_dir = tempdir().unwrap();
    let mut builder = MutsukiWebHost::builder()
        .application(MinimalWebApplication::empty("mutsuki.web.example"))
        .listen("127.0.0.1:0")
        .safe_mode(safe_mode)
        .shell_dir(shell_dir.path().join("shell"))
        .auth_token("test-token")
        .mode(DeploymentMode::Embedded);
    if let Some(extension) = extension {
        builder = builder.extension(extension);
    }
    let mut host = builder.build().expect("build host");
    host.start().await.expect("start host");
    // Keep tempdir alive by leaking — tests are short-lived.
    std::mem::forget(shell_dir);
    host
}

async fn http_get(addr: &str, path: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    (status, text)
}

#[tokio::test]
async fn starts_on_loopback_serves_shell_and_health() {
    let mut host = start_host(false, None).await;
    let addr = host.listen_addr().unwrap().to_string();
    assert!(addr.starts_with("127.0.0.1:"));

    let status = host.status();
    assert_eq!(status.status, WebHostStatus::Running);
    assert_eq!(status.protocol_version, WEB_PROTOCOL_VERSION);
    assert_eq!(status.mode, DeploymentMode::Embedded);

    let (code, body) = http_get(&addr, "/health").await;
    assert_eq!(code, 200);
    assert!(body.contains("\"healthy\":true"));

    let (code, body) = http_get(&addr, "/").await;
    assert_eq!(code, 200);
    assert!(body.contains("Recovery Shell") || body.contains("Mutsuki"));
    let headers = body.to_ascii_lowercase();
    assert!(
        headers.contains("content-security-policy:") && headers.contains("img-src 'self' data:"),
        "Host CSP must declare img-src so images do not fall back to default-src"
    );
    assert!(!headers.contains("qlogo"));

    host.stop().await.unwrap();
    assert_eq!(host.status().status, WebHostStatus::Stopped);
}

#[tokio::test]
async fn static_csp_includes_application_image_hosts() {
    let assets = tempdir().unwrap();
    std::fs::write(
        assets.path().join("index.html"),
        "<!doctype html><title>ok</title>",
    )
    .unwrap();
    let dest = tempdir().unwrap();
    let mut host = MutsukiWebHost::builder()
        .application(
            MinimalWebApplication::new(
                WebApplicationDescriptor {
                    id: "mutsuki.web.example".into(),
                    name: "Example".into(),
                    version: "0.1.0".into(),
                    brand: Some("Mutsuki".into()),
                    theme: Some("default".into()),
                },
                WebShellAssets {
                    root_dir: assets.path().to_path_buf(),
                    index_file: "index.html".into(),
                    import_map: Default::default(),
                },
            )
            .with_extra_img_src(vec!["https://*.qlogo.cn".into()]),
        )
        .listen("127.0.0.1:0")
        .mode(DeploymentMode::Embedded)
        .shell_dir(dest.path().join("shell"))
        .auth_token("test-token")
        .build()
        .expect("build host");
    host.start().await.expect("start host");
    let addr = host.listen_addr().unwrap().to_string();
    let (code, body) = http_get(&addr, "/").await;
    assert_eq!(code, 200);
    let headers = body.to_ascii_lowercase();
    assert!(
        headers.contains("img-src 'self' data: blob: https://*.qlogo.cn"),
        "application extra img-src must appear in the Host CSP header"
    );
    host.stop().await.unwrap();
    std::mem::forget(assets);
    std::mem::forget(dest);
}

#[tokio::test]
async fn static_csp_includes_extension_image_hosts() {
    let dir = tempdir().unwrap();
    let mut extension = ExampleExtension::ok(dir.path());
    extension.extra_img_src = vec!["https://*.qlogo.cn".into()];
    let mut host = start_host(false, Some(extension)).await;
    let addr = host.listen_addr().unwrap().to_string();
    let (code, body) = http_get(&addr, "/").await;
    assert_eq!(code, 200);
    assert!(
        body.to_ascii_lowercase()
            .contains("img-src 'self' data: blob: https://*.qlogo.cn"),
        "extension extra img-src must appear in the Host CSP header"
    );
    host.stop().await.unwrap();
}

#[tokio::test]
async fn loads_extension_and_answers_rpc_over_bridge() {
    let dir = tempdir().unwrap();
    let extension = ExampleExtension::ok(dir.path());
    let mut host = start_host(false, Some(extension)).await;
    let bridge = host.bridge().cloned().expect("bridge");

    let hello = bridge
        .handle_message(
            None,
            mutsuki_web_protocol::WireMessage::Hello {
                protocol_version: WEB_PROTOCOL_VERSION.into(),
                capabilities: vec!["example.read".into()],
                auth_token: Some("test-token".into()),
            },
        )
        .unwrap();
    let session_id = match hello {
        mutsuki_web_bridge::HandleOutcome::Reply(mutsuki_web_protocol::WireMessage::HelloAck {
            session,
            ..
        }) => session.session_id,
        _ => panic!("hello ack"),
    };

    let response = bridge
        .handle_message(
            Some(session_id),
            mutsuki_web_protocol::WireMessage::Rpc(mutsuki_web_protocol::RpcRequest {
                id: Uuid::new_v4(),
                namespace: "mutsuki.example.web".into(),
                method: "ping".into(),
                params: JsonValue::Null,
            }),
        )
        .unwrap();
    match response {
        mutsuki_web_bridge::HandleOutcome::Reply(mutsuki_web_protocol::WireMessage::RpcResult(
            result,
        )) => {
            assert!(result.error.is_none());
            assert_eq!(result.result.unwrap(), JsonValue::String("pong".into()));
        }
        _ => panic!("rpc result"),
    }

    let response = bridge
        .handle_message_async(
            Some(session_id),
            mutsuki_web_protocol::WireMessage::Rpc(mutsuki_web_protocol::RpcRequest {
                id: Uuid::new_v4(),
                namespace: "mutsuki.example.web".into(),
                method: "ping_async".into(),
                params: JsonValue::Null,
            }),
        )
        .await
        .unwrap();
    match response {
        mutsuki_web_bridge::HandleOutcome::Reply(mutsuki_web_protocol::WireMessage::RpcResult(
            result,
        )) => {
            assert!(result.error.is_none());
            assert_eq!(
                result.result.unwrap(),
                JsonValue::String("pong-async".into())
            );
        }
        _ => panic!("async rpc result"),
    }

    host.stop().await.unwrap();
    std::mem::forget(dir);
}

#[tokio::test]
async fn subscribed_idle_socket_receives_event_without_follow_up_request() {
    let dir = tempdir().unwrap();
    let extension = ExampleExtension::ok(dir.path());
    let mut host = start_host(false, Some(extension)).await;
    let addr = host.listen_addr().unwrap();
    let bridge = host.bridge().cloned().expect("bridge");
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("connect websocket");

    socket
        .send(WsMessage::Binary(
            mutsuki_web_protocol::WireMessage::Hello {
                protocol_version: WEB_PROTOCOL_VERSION.into(),
                capabilities: vec![],
                auth_token: Some("test-token".into()),
            }
            .encode()
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let hello = socket.next().await.unwrap().unwrap().into_data();
    assert!(matches!(
        mutsuki_web_protocol::WireMessage::decode(&hello).unwrap(),
        mutsuki_web_protocol::WireMessage::HelloAck { .. }
    ));

    let subscription_id = Uuid::new_v4();
    socket
        .send(WsMessage::Binary(
            mutsuki_web_protocol::WireMessage::Subscribe(mutsuki_web_protocol::EventSubscription {
                subscription_id,
                topic: "ticks".into(),
                required_capability: None,
            })
            .encode()
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    socket
        .send(WsMessage::Binary(
            mutsuki_web_protocol::WireMessage::Ping
                .encode()
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
    let pong = socket.next().await.unwrap().unwrap().into_data();
    assert!(matches!(
        mutsuki_web_protocol::WireMessage::decode(&pong).unwrap(),
        mutsuki_web_protocol::WireMessage::Pong
    ));

    bridge
        .publish_event("ticks", serde_json::json!({ "value": 7 }))
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("idle socket must receive event")
        .unwrap()
        .unwrap()
        .into_data();
    match mutsuki_web_protocol::WireMessage::decode(&event).unwrap() {
        mutsuki_web_protocol::WireMessage::Event(event) => {
            assert_eq!(event.subscription_id, subscription_id);
            assert_eq!(event.payload, serde_json::json!({ "value": 7 }));
        }
        other => panic!("expected event, got {other:?}"),
    }

    host.stop().await.unwrap();
}

#[tokio::test]
async fn extension_failure_is_isolated_and_recovery_still_works() {
    let mut host = start_host(true, Some(ExampleExtension::failing())).await;
    let status = host.status();
    assert!(status.safe_mode);
    assert!(status.extensions_failed >= 1);

    let bridge = host.bridge().cloned().unwrap();
    let hello = bridge
        .handle_message(
            None,
            mutsuki_web_protocol::WireMessage::Hello {
                protocol_version: WEB_PROTOCOL_VERSION.into(),
                capabilities: vec!["recovery.read".into(), "recovery.write".into()],
                auth_token: Some("test-token".into()),
            },
        )
        .unwrap();
    let session_id = match hello {
        mutsuki_web_bridge::HandleOutcome::Reply(mutsuki_web_protocol::WireMessage::HelloAck {
            session,
            ..
        }) => {
            assert!(session.safe_mode);
            session.session_id
        }
        _ => panic!("hello ack"),
    };

    let listed = bridge
        .handle_message(
            Some(session_id),
            mutsuki_web_protocol::WireMessage::Rpc(mutsuki_web_protocol::RpcRequest {
                id: Uuid::new_v4(),
                namespace: "recovery".into(),
                method: "list_failures".into(),
                params: JsonValue::Object(Default::default()),
            }),
        )
        .unwrap();
    match listed {
        mutsuki_web_bridge::HandleOutcome::Reply(mutsuki_web_protocol::WireMessage::RpcResult(
            result,
        )) => {
            assert!(result.error.is_none());
            let failures = result.result.unwrap();
            assert!(!failures.as_array().unwrap().is_empty());
        }
        _ => panic!("list failures"),
    }

    let addr = host.listen_addr().unwrap().to_string();
    let (code, body) = http_get(&addr, "/").await;
    assert_eq!(code, 200);
    assert!(body.contains("Recovery Shell"));

    host.stop().await.unwrap();
}

#[tokio::test]
async fn rejects_non_loopback_without_tls_or_remote_auth() {
    let err = MutsukiWebHost::builder()
        .application(MinimalWebApplication::empty("app"))
        .listen("0.0.0.0:8080")
        .build()
        .err()
        .expect("must fail");
    assert!(matches!(err, WebHostError::InvalidConfig(_)));
}

#[tokio::test]
async fn standalone_requires_link_endpoint() {
    let err = MutsukiWebHost::builder()
        .application(MinimalWebApplication::empty("app"))
        .listen("127.0.0.1:0")
        .mode(DeploymentMode::Standalone)
        .build()
        .err()
        .expect("must fail");
    assert!(matches!(err, WebHostError::InvalidConfig(_)));
}

#[tokio::test]
async fn standalone_mode_builds_with_link_endpoint() {
    let shell_dir = tempdir().unwrap();
    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::empty("app"))
        .listen("127.0.0.1:0")
        .mode(DeploymentMode::Standalone)
        .link_endpoint("local://mutsuki.servicehost")
        .shell_dir(shell_dir.path())
        .build()
        .unwrap();
    host.start().await.unwrap();
    assert_eq!(host.status().mode, DeploymentMode::Standalone);
    host.stop().await.unwrap();
    std::mem::forget(shell_dir);
}

#[tokio::test]
async fn capability_denies_missing_permission() {
    // Unauthenticated open_local is read-only: host.status works, recovery.write does not.
    let shell_dir = tempdir().unwrap();
    let mut host = MutsukiWebHost::builder()
        .application(MinimalWebApplication::empty("mutsuki.web.example"))
        .listen("127.0.0.1:0")
        .shell_dir(shell_dir.path().join("shell"))
        .mode(DeploymentMode::Embedded)
        .build()
        .unwrap();
    host.start().await.unwrap();
    let bridge = host.bridge().cloned().unwrap();
    let hello = bridge
        .handle_message(
            None,
            mutsuki_web_protocol::WireMessage::Hello {
                protocol_version: WEB_PROTOCOL_VERSION.into(),
                capabilities: vec![],
                auth_token: None,
            },
        )
        .unwrap();
    let session_id = match hello {
        mutsuki_web_bridge::HandleOutcome::Reply(mutsuki_web_protocol::WireMessage::HelloAck {
            session,
            ..
        }) => {
            assert!(
                !session.capabilities.iter().any(|cap| cap == "*"),
                "open_local must not grant *"
            );
            session.session_id
        }
        _ => panic!("hello"),
    };

    let response = bridge
        .handle_message(
            Some(session_id),
            mutsuki_web_protocol::WireMessage::Rpc(mutsuki_web_protocol::RpcRequest {
                id: Uuid::new_v4(),
                namespace: "host".into(),
                method: "status".into(),
                params: JsonValue::Null,
            }),
        )
        .unwrap();
    match response {
        mutsuki_web_bridge::HandleOutcome::Reply(mutsuki_web_protocol::WireMessage::RpcResult(
            result,
        )) => {
            let body = serde_json::to_string(&result).unwrap();
            assert!(!body.contains("token="));
            assert!(result.error.is_none());
        }
        _ => panic!("status"),
    }

    let denied = bridge
        .handle_message(
            Some(session_id),
            mutsuki_web_protocol::WireMessage::Rpc(mutsuki_web_protocol::RpcRequest {
                id: Uuid::new_v4(),
                namespace: "recovery".into(),
                method: "disable_extension".into(),
                params: JsonValue::Null,
            }),
        )
        .unwrap();
    match denied {
        mutsuki_web_bridge::HandleOutcome::Reply(mutsuki_web_protocol::WireMessage::RpcResult(
            result,
        )) => {
            assert_eq!(result.error.as_ref().unwrap().code, "capability_denied");
        }
        _ => panic!("expected capability denial"),
    }
    host.stop().await.unwrap();
    std::mem::forget(shell_dir);
}

#[tokio::test]
async fn slow_client_budget_does_not_block_stop() {
    let mut host = start_host(false, None).await;
    let bridge = host.bridge().cloned().unwrap();
    // Saturate event queues for a session; stop must still complete promptly.
    let hello = bridge
        .handle_message(
            None,
            mutsuki_web_protocol::WireMessage::Hello {
                protocol_version: WEB_PROTOCOL_VERSION.into(),
                capabilities: vec!["*".into()],
                auth_token: Some("test-token".into()),
            },
        )
        .unwrap();
    let session_id = match hello {
        mutsuki_web_bridge::HandleOutcome::Reply(mutsuki_web_protocol::WireMessage::HelloAck {
            session,
            ..
        }) => session.session_id,
        _ => panic!("hello"),
    };
    bridge
        .handle_message(
            Some(session_id),
            mutsuki_web_protocol::WireMessage::Subscribe(mutsuki_web_protocol::EventSubscription {
                subscription_id: Uuid::new_v4(),
                topic: "noise".into(),
                required_capability: None,
            }),
        )
        .unwrap();
    for _ in 0..512 {
        let _ = bridge.publish_event("noise", JsonValue::String("x".into()));
    }
    let started = std::time::Instant::now();
    host.stop().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
}
