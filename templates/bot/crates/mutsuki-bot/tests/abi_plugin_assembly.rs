use std::fs;
use std::process::Command;

use mutsuki_bot::assemble_service;
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_control::ControlMethod;
use mutsuki_service_ipc::{ControlClient, ControlClientConfig};
use mutsuki_service_plugin_loader::PluginToml;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

#[tokio::test]
async fn identical_business_config_runs_builtin_then_managed_abi() {
    let root = tempdir().unwrap();
    let metadata = Command::new(env!("CARGO"))
        .args(["metadata", "--locked", "--format-version", "1"])
        .output()
        .expect("read Cargo metadata");
    assert!(metadata.status.success());
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
    let plugin_manifest = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "mutsuki-plugin-bot-command")
        .and_then(|package| package["manifest_path"].as_str())
        .expect("Bot command manifest in dependency metadata");
    let fixture_target = root.path().join("plugin-target");
    let build = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path", plugin_manifest, "-p"])
        .arg("mutsuki-plugin-bot-command")
        .arg("--target-dir")
        .arg(&fixture_target)
        .status()
        .expect("build Bot command ABI artifact");
    assert!(build.success());
    let file_name = if cfg!(target_os = "windows") {
        "mutsuki_plugin_bot_command.dll"
    } else if cfg!(target_os = "macos") {
        "libmutsuki_plugin_bot_command.dylib"
    } else {
        "libmutsuki_plugin_bot_command.so"
    };
    let artifact = fixture_target.join("debug").join(file_name);
    assert!(artifact.is_file(), "ABI plugin: {}", artifact.display());

    let installed = root.path().join("plugins").join("installed");
    let plugin_dir = installed.join("mutsuki.bot.command");
    fs::create_dir_all(&plugin_dir).unwrap();
    let installed_artifact = plugin_dir.join(file_name);
    fs::copy(&artifact, &installed_artifact).unwrap();
    let sha256 = format!(
        "sha256:{:x}",
        Sha256::digest(fs::read(&installed_artifact).unwrap())
    );
    let manifest = mutsuki_plugin_bot_command::bot_command_abi_manifest(file_name, &sha256);
    fs::write(
        plugin_dir.join("plugin.toml"),
        toml::to_string(&PluginToml {
            manifest,
            runtime: None,
        })
        .unwrap(),
    )
    .unwrap();

    let config_path = root.path().join("product.toml");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let control_addr = listener.local_addr().unwrap();
    drop(listener);
    fs::write(
        &config_path,
        format!(
            r#"[service]
profile = "test"
instance_id = "template-abi-test"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = true
transport = "tcp-debug"
name = "template-abi-test"
token = "test-token"
tcp_debug_addr = "{}"

[plugins]
dynamic_dirs = ["{}"]
disabled_dir = "plugins/disabled"

[[plugins.configured]]
id = "mutsuki.bot.command"

[plugins.configured.config]
prefixes = ["/"]

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
            root.path().to_string_lossy().replace('\\', "/"),
            control_addr,
            installed.to_string_lossy().replace('\\', "/"),
        ),
    )
    .unwrap();
    let service = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path),
        ..Default::default()
    })
    .unwrap();

    let runtime = assemble_service(service.clone())
        .unwrap()
        .start()
        .await
        .unwrap();
    runtime.shutdown().await;
    let builtin_lock: serde_json::Value = serde_json::from_slice(
        &fs::read(root.path().join("run").join("runtime.lock.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        builtin_lock["plugin_deployments"]["mutsuki.bot.command"],
        "builtin"
    );

    let runtime = assemble_service(service.clone())
        .unwrap()
        .start()
        .await
        .unwrap();
    let client = ControlClient::new(ControlClientConfig::from(&service));
    let switched = client
        .request(
            ControlMethod::PluginDeploymentSet,
            serde_json::json!({
                "plugin_id": "mutsuki.bot.command",
                "deployment": "abi"
            }),
        )
        .await
        .unwrap();
    assert!(switched.ok, "switch ABI: {:?}", switched.error);
    let abi_lock: serde_json::Value = serde_json::from_slice(
        &fs::read(root.path().join("run").join("runtime.lock.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(abi_lock["plugin_deployments"]["mutsuki.bot.command"], "abi");
    runtime.shutdown().await;

    let runtime = assemble_service(service.clone())
        .unwrap()
        .start()
        .await
        .unwrap();
    let abi_lock: serde_json::Value = serde_json::from_slice(
        &fs::read(root.path().join("run").join("runtime.lock.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(abi_lock["plugin_deployments"]["mutsuki.bot.command"], "abi");
    let client = ControlClient::new(ControlClientConfig::from(&service));
    let cleared = client
        .request(
            ControlMethod::PluginDeploymentClear,
            serde_json::json!({ "plugin_id": "mutsuki.bot.command" }),
        )
        .await
        .unwrap();
    assert!(cleared.ok, "clear deployment: {:?}", cleared.error);
    let cleared_lock: serde_json::Value = serde_json::from_slice(
        &fs::read(root.path().join("run").join("runtime.lock.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        cleared_lock["plugin_deployments"]["mutsuki.bot.command"],
        "builtin"
    );
    runtime.shutdown().await;
}
