//! Process-level smoke: `mutsuki-bot-console` binary + Runtime over quic:// Link.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use mutsuki_bot::assemble_service;
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standalone_console_binary_reaches_runtime_health_over_quic() {
    let root = tempdir().unwrap();
    let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert_pem = generated.cert.pem();
    let key_pem = generated.key_pair.serialize_pem();

    let secret_path = root.path().join("product.secret.toml");
    std::fs::write(
        &secret_path,
        format!(
            "[secrets]\nWEB_CONSOLE_AUTH_TOKEN = \"test-token\"\nLINK_QUIC_CERT_PEM = \"{cert}\"\nLINK_QUIC_KEY_PEM = \"{key}\"\nLINK_QUIC_CA_CERT_PEM = \"{cert}\"\n",
            cert = escape_toml(&cert_pem),
            key = escape_toml(&key_pem),
        ),
    )
    .unwrap();

    let runtime_config_path = root.path().join("runtime.toml");
    std::fs::write(
        &runtime_config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[security]
secret_file = "{secret}"

[link.quic]
enabled = true
listen = "127.0.0.1:0"
cert_pem_key = "LINK_QUIC_CERT_PEM"
key_pem_key = "LINK_QUIC_KEY_PEM"
"#,
                secret = secret_path.to_string_lossy().replace('\\', "/")
            ),
        ),
    )
    .unwrap();

    let service = load(&runtime_config_path);
    let runtime = assemble_service(service).unwrap().start().await.unwrap();
    let quic_addr = runtime.quic_link_addr().expect("quic link listener");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let console_config_path = root.path().join("console.toml");
    std::fs::write(
        &console_config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[security]
secret_file = "{secret}"

[web.console]
enabled = true
listen = "127.0.0.1:0"
auth_token_key = "WEB_CONSOLE_AUTH_TOKEN"
link_endpoint = "quic://{quic_addr}"
quic_server_name = "localhost"
quic_ca_cert_key = "LINK_QUIC_CA_CERT_PEM"
"#,
                secret = secret_path.to_string_lossy().replace('\\', "/"),
                quic_addr = quic_addr,
            ),
        ),
    )
    .unwrap();

    let log_path = root.path().join("console.log");
    let mut console = ConsoleProcess::spawn(&console_config_path, log_path);
    let addr = console.wait_for_listen_addr(Duration::from_secs(30)).await;

    let health = ws_rpc(&addr, "control", "health", "test-token")
        .await
        .expect("health");
    assert_eq!(health["service"], "ok");

    let status = ws_rpc(&addr, "control", "service_status", "test-token")
        .await
        .expect("service_status");
    assert_eq!(status["instance_id"], "template-console-test");

    console.kill();
    runtime.shutdown().await;
}

struct ConsoleProcess {
    child: Child,
    log_path: PathBuf,
}

impl ConsoleProcess {
    fn spawn(config_path: &Path, log_path: PathBuf) -> Self {
        let output = File::create(&log_path).expect("create console process log");
        let error = output.try_clone().expect("clone console process log");
        let child = Command::new(env!("CARGO_BIN_EXE_mutsuki-bot-console"))
            .arg(config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::from(output))
            .stderr(Stdio::from(error))
            .spawn()
            .expect("start mutsuki-bot-console binary");
        Self { child, log_path }
    }

    async fn wait_for_listen_addr(&mut self, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("inspect console process") {
                panic!(
                    "console exited before listening with {status}: {}",
                    self.diagnostics()
                );
            }
            let log = self.diagnostics();
            if let Some(addr) = parse_listen_addr(&log) {
                return addr;
            }
            if Instant::now() >= deadline {
                panic!("console did not publish listen address: {log}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn diagnostics(&self) -> String {
        std::fs::read_to_string(&self.log_path)
            .unwrap_or_else(|error| format!("failed to read process log: {error}"))
    }
}

impl Drop for ConsoleProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn parse_listen_addr(log: &str) -> Option<String> {
    const MARKER: &str = "listening on http://";
    let line = log.lines().rev().find(|line| line.contains(MARKER))?;
    let addr = line.split(MARKER).nth(1)?.trim();
    if addr.is_empty() {
        None
    } else {
        Some(addr.to_string())
    }
}

async fn ws_rpc(
    addr: &str,
    namespace: &str,
    method: &str,
    auth_token: &str,
) -> Result<serde_json::Value, String> {
    use futures_util::{SinkExt, StreamExt};
    use mutsuki_web_protocol::{RpcRequest, WEB_PROTOCOL_VERSION, WireMessage};
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    use uuid::Uuid;

    let (mut ws, _) = connect_async(format!("ws://{addr}/ws"))
        .await
        .map_err(|err| err.to_string())?;
    ws.send(Message::Text(
        serde_json::to_string(&WireMessage::Hello {
            protocol_version: WEB_PROTOCOL_VERSION.into(),
            capabilities: vec!["runtime.read".into(), "*".into()],
            auth_token: Some(auth_token.into()),
        })
        .unwrap()
        .into(),
    ))
    .await
    .map_err(|err| err.to_string())?;
    let Message::Text(ack) = ws.next().await.unwrap().unwrap() else {
        return Err("missing hello ack".into());
    };
    assert!(matches!(
        serde_json::from_str::<WireMessage>(&ack).unwrap(),
        WireMessage::HelloAck { .. }
    ));
    let id = Uuid::new_v4();
    ws.send(Message::Text(
        serde_json::to_string(&WireMessage::Rpc(RpcRequest {
            id,
            namespace: namespace.into(),
            method: method.into(),
            params: serde_json::json!({"capabilities": ["runtime.read", "*"]}),
        }))
        .unwrap()
        .into(),
    ))
    .await
    .map_err(|err| err.to_string())?;
    let Message::Text(text) = ws.next().await.unwrap().unwrap() else {
        return Err("missing rpc result".into());
    };
    match serde_json::from_str::<WireMessage>(&text).unwrap() {
        WireMessage::RpcResult(result) => match result.error {
            Some(error) => Err(error.message),
            None => Ok(result.result.unwrap_or(serde_json::Value::Null)),
        },
        other => Err(format!("unexpected wire message: {other:?}")),
    }
}

fn load(path: &Path) -> ServiceConfig {
    ServiceConfig::load(ConfigOverrides {
        config_file: Some(path.to_path_buf()),
        ..Default::default()
    })
    .unwrap()
}

fn escape_toml(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn service_toml(root: &Path, extra: &str) -> String {
    format!(
        r#"[service]
profile = "test"
instance_id = "template-console-test"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "named-pipe"
name = "template-console-test"
token = "test-token"

[observe]
console = false

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"
{}
"#,
        root.to_string_lossy().replace('\\', "/"),
        extra
    )
}
