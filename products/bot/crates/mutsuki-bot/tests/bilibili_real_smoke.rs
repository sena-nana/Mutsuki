//! Real-account Bilibili smoke through the production binary: QR login over the Web Console,
//! then an authenticated dynamic preview. Every upstream call goes through the std HTTP effect,
//! and the captured login cookie must never reach the process output or logs.
//!
//! Run with a human at hand to scan the QR code:
//! `MUTSUKI_BILIBILI_QR_PNG=/path/qr.png cargo test --locked -p mutsuki-bot --test
//! bilibili_real_smoke -- --ignored --nocapture`. `MUTSUKI_BILIBILI_SMOKE_UID` picks the account
//! previewed after login (default 2).

#[path = "support/process.rs"]
mod process;

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use mutsuki_bot::{
    CONSOLE_AUTH_TOKEN_KEY, MEDIA_RESOURCE_PROVIDER_ID, PRODUCT_CONFIG_PROVIDER_ID,
    load_single_instance_product_for_test,
};
use mutsuki_config_service::{ConfigApplyRequest, ConfigContext, ConfigValue};
use mutsuki_web_protocol::{RpcRequest, WEB_PROTOCOL_VERSION, WireMessage};
use serde_json::{Value, json};
use uuid::Uuid;

use process::ProductProcess;

const BILIBILI_PLUGIN_ID: &str = "mutsuki.bot.bilibili";
const IMAGE_RENDER_PLUGIN_ID: &str = "mutsuki.std.image.render";
const COOKIE_SECRET_KEY: &str = "BILIBILI_COOKIE";
const SMOKE_CONSOLE_TOKEN: &str = "bilibili-smoke-console-token";
const SCAN_TIMEOUT: Duration = Duration::from_secs(600);
/// A fresh QR code replaces an expired one this many times before the smoke gives up.
const QR_ATTEMPTS: usize = 3;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a person to scan the Bilibili QR code"]
async fn real_bilibili_qr_login_captures_cookie_and_previews_through_http_effect() {
    let qr_path = PathBuf::from(
        std::env::var("MUTSUKI_BILIBILI_QR_PNG").expect("set MUTSUKI_BILIBILI_QR_PNG"),
    );
    let uid = std::env::var("MUTSUKI_BILIBILI_SMOKE_UID").unwrap_or_else(|_| "2".into());
    let root = tempfile::Builder::new()
        .prefix("mtk-bilibili-real-")
        .tempdir()
        .expect("create smoke root");
    let executable = copy_product_executable(root.path());
    let instance_root = root.path().join(".mutsuki-bot");
    std::fs::create_dir_all(&instance_root).unwrap();
    std::fs::write(
        instance_root.join("secrets.toml"),
        format!("[secrets]\n\"{CONSOLE_AUTH_TOKEN_KEY}\" = \"{SMOKE_CONSOLE_TOKEN}\"\n"),
    )
    .unwrap();
    enable_bilibili_and_image_render(&instance_root).await;

    let listen = free_loopback_listen();
    let mut process = ProductProcess::spawn(&executable, &listen, root.path().join("product.log"));
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            process.assert_running();
            if rpc(&listen, "bilibili", "status", json!({})).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "Bilibili console never came up; {}",
            process.output_summary()
        )
    });

    wait_for_qr_confirmation(&listen, &mut process, &qr_path).await;

    let status = rpc(&listen, "bilibili", "status", json!({})).await.unwrap();
    assert_eq!(status["cookie_secret_state"], "present", "{status}");
    assert_eq!(status["credential_loaded"], true, "{status}");

    rpc(
        &listen,
        "bilibili",
        "subscriptions.subscribe",
        json!({
            "subscription_id": "smoke",
            "uid": uid.parse::<u64>().expect("numeric smoke UID"),
            "notifications": ["dynamic"],
            "target": {"type": "group", "group_id": "smoke"},
            "outbound_binding": "smoke",
        }),
    )
    .await
    .expect("subscribe the smoke account");
    let preview = rpc(
        &listen,
        "bilibili",
        "subscriptions.preview",
        json!({"selector": "smoke"}),
    )
    .await
    .expect("authenticated dynamic preview");
    eprintln!("preview: {} {}", preview["title"], preview["url"]);
    assert!(
        preview["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("https://"))
    );

    rpc(&listen, "control", "service_shutdown", json!({}))
        .await
        .expect("request ServiceShutdown");
    let exit = process.wait_for_exit(Duration::from_secs(30)).await;
    assert!(exit.success(), "product exited with {exit}");

    let cookie = stored_cookie(&instance_root);
    assert!(
        cookie.contains("SESSDATA="),
        "captured cookie lacks SESSDATA"
    );
    let mut observed = process.output_bytes();
    for entry in walk(&instance_root.join("logs")) {
        observed.extend(std::fs::read(entry).unwrap_or_default());
    }
    for pair in cookie.split("; ") {
        let value = pair.split_once('=').map_or(pair, |(_, value)| value);
        assert!(
            value.len() < 8 || !contains(&observed, value.as_bytes()),
            "a captured cookie value reached the product output or logs"
        );
    }
}

/// Shows a QR code and polls until the person confirms the scan. An expired code is replaced
/// with a fresh one at the same path, up to [`QR_ATTEMPTS`] codes.
async fn wait_for_qr_confirmation(listen: &str, process: &mut ProductProcess, qr_path: &Path) {
    let deadline = tokio::time::Instant::now() + SCAN_TIMEOUT;
    for attempt in 1..=QR_ATTEMPTS {
        let login = rpc(listen, "bilibili", "login.start", json!({}))
            .await
            .expect("start QR login");
        let png = decode_base64(login["qr_png_base64"].as_str().expect("QR PNG"));
        std::fs::write(qr_path, png).expect("write QR PNG");
        eprintln!(
            "QR code {attempt}/{QR_ATTEMPTS} written to {}; scan it with the Bilibili app",
            qr_path.display()
        );
        let mut last = String::new();
        loop {
            process.assert_running();
            assert!(
                tokio::time::Instant::now() < deadline,
                "QR login was not confirmed in time"
            );
            let polled = rpc(listen, "bilibili", "login.poll", json!({}))
                .await
                .unwrap_or_else(|error| panic!("poll QR login: {error}"));
            let status = polled["status"].as_str().unwrap_or_default().to_owned();
            if status != last {
                eprintln!("QR login status: {status} ({})", polled["message"]);
                last.clone_from(&status);
            }
            match status.as_str() {
                "confirmed" => return,
                "expired" => break,
                _ => tokio::time::sleep(Duration::from_secs(2)).await,
            }
        }
    }
    panic!("all {QR_ATTEMPTS} QR codes expired before one was confirmed");
}

async fn enable_bilibili_and_image_render(instance_root: &std::path::Path) {
    let product = load_single_instance_product_for_test(instance_root, "unused")
        .await
        .expect("seed single-instance product");
    let update = |provider: &'static str, edit: Box<dyn FnOnce(&mut ConfigValue) + Send>| {
        let config = product.config.clone();
        async move {
            let snapshot = config
                .read(provider, ConfigContext::global(), &["*".into()])
                .await
                .expect("read owner config");
            let mut candidate = snapshot.value;
            edit(&mut candidate);
            config
                .apply(
                    provider,
                    ConfigApplyRequest {
                        candidate,
                        expected_revision: snapshot.revision,
                        dry_run: false,
                    },
                    ConfigContext::global(),
                    &["*".into()],
                )
                .await
                .unwrap_or_else(|error| panic!("apply {provider}: {error}"));
        }
    };
    update(
        BILIBILI_PLUGIN_ID,
        Box::new(|value| {
            let ConfigValue::Object(object) = value else {
                panic!("Bilibili config must be an object")
            };
            object.insert("enabled".into(), ConfigValue::Bool(true));
            object.insert("management_enabled".into(), ConfigValue::Bool(true));
            object.insert(
                "management_self_binding_outbound_binding".into(),
                ConfigValue::String("smoke".into()),
            );
        }),
    )
    .await;
    update(
        PRODUCT_CONFIG_PROVIDER_ID,
        Box::new(|value| {
            let ConfigValue::Object(object) = value else {
                panic!("product config must be an object")
            };
            let ConfigValue::Object(plugins) = object
                .entry("runtime_plugins".to_owned())
                .or_insert_with(|| ConfigValue::Object(std::collections::BTreeMap::default()))
            else {
                panic!("runtime_plugins must be an object")
            };
            plugins.insert(
                IMAGE_RENDER_PLUGIN_ID.into(),
                ConfigValue::from_json(&json!({
                    "enabled": true,
                    "config": {"output_provider_id": MEDIA_RESOURCE_PROVIDER_ID},
                })),
            );
        }),
    )
    .await;
}

/// Reads the captured cookie. Failures name nothing from the file: a parser error would
/// otherwise echo the secret file into the test output.
fn stored_cookie(instance_root: &std::path::Path) -> String {
    std::fs::read_to_string(instance_root.join("secrets.toml"))
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|table| {
            table
                .get("secrets")?
                .get(COOKIE_SECRET_KEY)?
                .as_str()
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| panic!("QR login did not store a readable {COOKIE_SECRET_KEY} secret"))
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                walk(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn decode_base64(text: &str) -> Vec<u8> {
    let value = |byte: u8| match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("invalid base64"),
    };
    let mut out = Vec::new();
    for chunk in text.trim_end_matches('=').as_bytes().chunks(4) {
        let mut bits = 0_u32;
        for (index, byte) in chunk.iter().enumerate() {
            bits |= u32::from(value(*byte)) << (18 - 6 * index);
        }
        let bytes = bits.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    out
}

fn copy_product_executable(root: &std::path::Path) -> PathBuf {
    let source = std::path::Path::new(env!("CARGO_BIN_EXE_mutsuki-bot"));
    let target = root.join(source.file_name().unwrap());
    std::fs::copy(source, &target).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    target
}

fn free_loopback_listen() -> String {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string()
}

async fn rpc(address: &str, namespace: &str, method: &str, params: Value) -> Result<Value, String> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let (mut socket, _) = connect_async(format!("ws://{address}/ws"))
        .await
        .map_err(|error| error.to_string())?;
    socket
        .send(Message::Binary(
            WireMessage::Hello {
                protocol_version: WEB_PROTOCOL_VERSION.into(),
                capabilities: Vec::new(),
                auth_token: Some(SMOKE_CONSOLE_TOKEN.into()),
            }
            .encode()
            .map_err(|error| error.to_string())?
            .into(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    let _ = socket.next().await.ok_or("missing hello ack")?;
    socket
        .send(Message::Binary(
            WireMessage::Rpc(RpcRequest {
                id: Uuid::new_v4(),
                namespace: namespace.into(),
                method: method.into(),
                params,
            })
            .encode()
            .map_err(|error| error.to_string())?
            .into(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    loop {
        let message = socket
            .next()
            .await
            .ok_or("missing RPC response")?
            .map_err(|error| error.to_string())?;
        let Message::Binary(bytes) = message else {
            continue;
        };
        if let WireMessage::RpcResult(result) =
            WireMessage::decode(bytes.as_ref()).map_err(|error| error.to_string())?
        {
            return result.error.map_or_else(
                || Ok(result.result.unwrap_or_default()),
                |error| Err(format!("{}: {}", error.code, error.message)),
            );
        }
    }
}
