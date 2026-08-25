use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mutsuki_service_config::{IpcCodec, IpcTransport, ServiceConfig};
use mutsuki_service_control::{
    ControlCommand, ControlHandler, ControlMethod, ControlRequest, ControlResponse, ControlResult,
    HealthReport,
};
use tokio::sync::Mutex;

use super::*;
use crate::frame::{BINARY_LENGTH_PREFIX_LEN, FrameFlags, encode_frame};

struct OkHandler;

fn healthy_report() -> HealthReport {
    HealthReport {
        service: "healthy".into(),
        core: "healthy".into(),
        plugins: "healthy".into(),
        runners: "healthy".into(),
        event_sources: "healthy".into(),
        event_source_details: Vec::new(),
        recent_errors: Vec::new(),
        components: Default::default(),
    }
}

impl ControlHandler for OkHandler {
    fn handle(&self, _request: ControlRequest) -> mutsuki_service_control::ControlFuture {
        Box::pin(async { ControlResponse::ok(ControlResult::HealthCheck(healthy_report())) })
    }
}

struct SlowHandler {
    started: Arc<Mutex<u32>>,
}

impl ControlHandler for SlowHandler {
    fn handle(&self, request: ControlRequest) -> mutsuki_service_control::ControlFuture {
        let started = self.started.clone();
        Box::pin(async move {
            *started.lock().await += 1;
            if request.method() == ControlMethod::HealthCheck {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            ControlResponse::ok(ControlResult::HealthCheck(healthy_report()))
        })
    }
}

#[test]
fn endpoint_helper_is_transport_specific() {
    let run_dir = Path::new("runtime");
    assert_eq!(
        default_control_endpoint(IpcTransport::NamedPipe, "mutsuki", run_dir, None),
        "mutsuki"
    );
    assert!(
        default_control_endpoint(IpcTransport::UnixSocket, "mutsuki", run_dir, None)
            .ends_with("mutsuki.sock")
    );
    assert_eq!(
        default_control_endpoint(
            IpcTransport::TcpDebug,
            "mutsuki",
            run_dir,
            Some("127.0.0.1:9000")
        ),
        "127.0.0.1:9000"
    );
}

#[test]
fn control_method_opcodes_are_stable() {
    assert_eq!(ControlMethod::HealthCheck.opcode(), 0x0013);
    assert_eq!(
        ControlMethod::from_opcode(0x0013),
        Some(ControlMethod::HealthCheck)
    );
    assert_eq!(ControlMethod::RuntimeStatistics.opcode(), 0x0017);
    assert_eq!(
        ControlMethod::from_opcode(0x0017),
        Some(ControlMethod::RuntimeStatistics)
    );
    assert_eq!(ControlMethod::HostMetrics.opcode(), 0x0018);
    assert_eq!(
        ControlMethod::from_opcode(0x0018),
        Some(ControlMethod::HostMetrics)
    );
    assert!(!ControlMethod::HostMetrics.is_mutating());
    assert!(ControlMethod::PluginReload.is_mutating());
    assert!(!ControlMethod::HealthCheck.is_mutating());
    assert!(!ControlMethod::RuntimeStatistics.is_mutating());
}

#[tokio::test]
async fn binary_rejects_oversized_length_prefix_before_payload_alloc() {
    let limits = ControlIpcLimits {
        max_frame_bytes: 128,
        max_payload_bytes: 64,
        ..ControlIpcLimits::default()
    };
    let oversized = (limits.max_frame_bytes as u32 + 1).to_be_bytes();
    let err = crate::frame::validate_frame_length(u32::from_be_bytes(oversized) as usize, limits)
        .expect_err("oversized");
    assert!(matches!(err, IpcError::FrameOversized { .. }));
}

#[tokio::test]
async fn truncated_binary_frame_fails() {
    let limits = ControlIpcLimits::default();
    let bytes = encode_frame(
        ControlMethod::HealthCheck.opcode(),
        FrameFlags::REQUEST,
        1,
        vec![1, 2, 3, 4],
        limits,
    )
    .unwrap();
    let truncated = &bytes[..BINARY_LENGTH_PREFIX_LEN + 8];
    let err = crate::frame::decode_binary_frame(truncated, limits).expect_err("truncated");
    assert!(matches!(err, IpcError::Truncated { .. }));
}

#[cfg(unix)]
#[tokio::test]
async fn unix_server_shutdown_removes_socket_path() {
    let root = tempfile::tempdir().unwrap();
    let mut config = ServiceConfig::default();
    config.service.run_dir = root.path().to_path_buf();
    config.ipc.enabled = true;
    config.ipc.transport = IpcTransport::UnixSocket;
    config.ipc.codec = IpcCodec::Binary;
    config.ipc.name = "ipc-cleanup".into();
    let endpoint = std::path::PathBuf::from(config.ipc_endpoint());

    let server = start_server(&config, Arc::new(OkHandler))
        .await
        .unwrap()
        .unwrap();
    assert!(endpoint.exists());
    server.shutdown().await;
    assert!(!endpoint.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn persistent_binary_handles_multiple_requests_on_one_connection() {
    let root = tempfile::tempdir().unwrap();
    let mut config = ServiceConfig::default();
    config.service.run_dir = root.path().to_path_buf();
    config.ipc.enabled = true;
    config.ipc.transport = IpcTransport::UnixSocket;
    config.ipc.codec = IpcCodec::Binary;
    config.ipc.name = "ipc-persistent".into();
    config.ipc.token = Some("tok".into());

    let server = start_server(&config, Arc::new(OkHandler))
        .await
        .unwrap()
        .unwrap();
    let client = ControlClient::new((&config).into());
    let session = ControlSession::connect(client.config().clone())
        .await
        .unwrap();
    for _ in 0..8 {
        let response = session.request(ControlCommand::HealthCheck).await.unwrap();
        assert!(response.is_ok());
    }
    assert_eq!(session.connection_count(), 1);
    session.close().await.unwrap();
    server.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn binary_multiplex_cancel_and_timeout() {
    let root = tempfile::tempdir().unwrap();
    let mut config = ServiceConfig::default();
    config.service.run_dir = root.path().to_path_buf();
    config.ipc.enabled = true;
    config.ipc.transport = IpcTransport::UnixSocket;
    config.ipc.codec = IpcCodec::Binary;
    config.ipc.name = "ipc-cancel".into();
    config.ipc.token = Some("tok".into());
    config.ipc.request_timeout_ms = 50;

    let started = Arc::new(Mutex::new(0_u32));
    let server = start_server(
        &config,
        Arc::new(SlowHandler {
            started: started.clone(),
        }),
    )
    .await
    .unwrap()
    .unwrap();
    let session = ControlSession::connect((&config).into()).await.unwrap();
    let err = session
        .request(ControlCommand::HealthCheck)
        .await
        .expect_err("timeout");
    assert!(matches!(err, IpcError::Timeout));
    session.close().await.unwrap();
    server.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn drain_rejects_new_requests() {
    let root = tempfile::tempdir().unwrap();
    let mut config = ServiceConfig::default();
    config.service.run_dir = root.path().to_path_buf();
    config.ipc.enabled = true;
    config.ipc.transport = IpcTransport::UnixSocket;
    config.ipc.codec = IpcCodec::Binary;
    config.ipc.name = "ipc-drain".into();
    config.ipc.token = Some("tok".into());

    let server = start_server(&config, Arc::new(OkHandler))
        .await
        .unwrap()
        .unwrap();
    ControlSession::connect((&config).into())
        .await
        .expect("the server accepts sessions before it drains");

    server.begin_drain();
    // Drain is observed by the accept loop, so the reject becomes visible a moment after the call
    // returns. Polling for it asserts the outcome instead of guessing how long that moment is.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let rejected = match ControlSession::connect((&config).into()).await {
            Err(_) => true,
            // Losing the race with the accept loop is allowed; being served is not.
            Ok(session) => {
                let served = session.request(ControlCommand::HealthCheck).await.is_ok();
                let _ = session.close().await;
                !served
            }
        };
        if rejected {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "a draining server kept serving new sessions"
        );
        tokio::task::yield_now().await;
    }
    server.shutdown().await;
}

#[cfg(windows)]
#[tokio::test]
async fn named_pipe_server_is_ready_when_start_returns() {
    let mut config = ServiceConfig::default();
    config.ipc.enabled = true;
    config.ipc.transport = IpcTransport::NamedPipe;
    config.ipc.codec = IpcCodec::Binary;
    config.ipc.name = format!("mutsuki-ipc-ready-{}", std::process::id());
    config.ipc.token = Some("test-token".into());

    let server = start_server(&config, Arc::new(OkHandler))
        .await
        .unwrap()
        .unwrap();
    let response = ControlClient::new((&config).into())
        .request(ControlCommand::HealthCheck)
        .await
        .unwrap();

    assert!(response.is_ok());
    server.shutdown().await;
}

#[test]
fn encode_helpers_reuse_caller_buffers() {
    let limits = ControlIpcLimits::default();
    let request = ControlRequest::new("t", ControlCommand::HealthCheck);
    let mut frame = Vec::with_capacity(64);
    let mut payload = Vec::with_capacity(64);
    crate::codec::encode_binary_request_with_scratch(1, &request, limits, &mut frame, &mut payload)
        .unwrap();
    assert!(frame.len() > BINARY_LENGTH_PREFIX_LEN);
    assert!(!frame.ends_with(b"\n"));
}
