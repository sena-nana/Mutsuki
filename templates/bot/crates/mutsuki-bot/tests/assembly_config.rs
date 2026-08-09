use std::path::Path;

use mutsuki_bot::{apply_product_runtime_profile, assemble_service};
use mutsuki_bot_service_host_integration::{BilibiliConsoleBridge, configured_bot_plugin_catalog};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, PluginManifest, ProtocolClass, RunnerContext,
    RunnerDescriptor, RunnerPurity, RuntimeError, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_runtime::{
    ConfiguredPluginFactory, ServiceRuntimeBuilder, ServiceRuntimeResult,
};
use mutsuki_std_service_host_integration::configured_std_plugin_catalog;
use serde_json::Value;
use tempfile::tempdir;

const SIMPLE_TEMPLATE: &str = include_str!("../../../config/template.toml");

#[tokio::test]
async fn empty_external_config_starts_and_stops_neutral_runtime() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(&config_path, service_toml(root.path(), "")).unwrap();
    let service = load(&config_path);

    let runtime = assemble_service(service).unwrap().start().await.unwrap();
    runtime.shutdown().await;
}

#[tokio::test]
async fn bot_profile_starts_with_isolated_execution_paths() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let mut product = service_toml(root.path(), "");
    product = product.replacen("profile = \"test\"", "profile = \"bot\"", 1);
    std::fs::write(&config_path, product).unwrap();
    let mut service = load(&config_path);
    apply_product_runtime_profile(&mut service);

    let domains = &service.core.execution_domains;
    assert_eq!(domains.len(), 4);
    assert!(domains.iter().any(|domain| domain.id == "bot-control"));
    assert!(domains.iter().any(|domain| domain.id == "network-io"));
    assert!(
        domains
            .iter()
            .any(|domain| domain.id == "blocking-adapters")
    );
    assert!(domains.iter().any(|domain| domain.id == "agent-compute"));

    let runtime = assemble_service(service).unwrap().start().await.unwrap();
    runtime.shutdown().await;
}

#[tokio::test]
async fn unknown_configured_plugin_fails_loud() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            r#"
[[plugins.configured]]
id = "owner.plugin.not-linked"
"#,
        ),
    )
    .unwrap();

    let error = match assemble_service(load(&config_path)).unwrap().start().await {
        Ok(runtime) => {
            runtime.shutdown().await;
            panic!("unknown configured plugin started")
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("owner.plugin.not-linked"));
}

#[tokio::test]
async fn configured_qq_integration_fails_preflight_without_host_secret() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            r#"
[[plugins.configured]]
id = "mutsuki.bot.adapter.qqbot"

[plugins.configured.config]
account_id = "configured-account"
app_id = "configured-app"
client_secret_key = "MISSING_TEMPLATE_QQ_SECRET"
"#,
        ),
    )
    .unwrap();
    let service = load(&config_path);

    let error = match assemble_service(service).unwrap().start().await {
        Ok(runtime) => {
            runtime.shutdown().await;
            panic!("QQ integration started without required Host secret")
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("MISSING_TEMPLATE_QQ_SECRET"));
}

#[tokio::test]
async fn bilibili_management_starts_with_host_owned_persistence_boundaries() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let secret_path = root.path().join("product.secret.toml");
    std::fs::write(&secret_path, "[secrets]\n").unwrap();
    let secret_path = secret_path.to_string_lossy().replace('\\', "/");
    let font = test_font_path();
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[security]
secret_file = "{secret_path}"

[[plugins.configured]]
id = "mutsuki.std.resource.memory"

[[plugins.configured]]
id = "mutsuki.std.image.render"

[plugins.configured.config]
output_provider_id = "mutsuki.std.resource.memory"
font_files = ["{font}"]

[[plugins.configured]]
id = "mutsuki.bot.command"

[plugins.configured.config]
prefixes = ["/"]

[[plugins.configured]]
id = "mutsuki.bot.bilibili"

[plugins.configured.config]
backend = {{ type = "web_cookie", cookie_secret_key = "BILIBILI_COOKIE" }}
live_interval_ms = 60000
dynamic_interval_ms = 60000
video_interval_ms = 60000
retry = {{ max_attempts = 3, initial_backoff_ms = 100, max_backoff_ms = 1000 }}
subscriptions = []
link_resolver = {{ enabled = false, cooldown_ms = 1000, account_to_binding = {{}} }}
media_provider_id = "mutsuki.std.resource.memory"
management = {{ enabled = true, allow_self_binding = true, command = "bili", admin_user_ids = ["admin"], self_binding_notifications = ["dynamic"], self_binding_outbound_binding = "qq-main" }}
"#,
            ),
        ),
    )
    .unwrap();

    let runtime = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .unwrap();
    let bridge = BilibiliConsoleBridge::get(&runtime).expect("bilibili console bridge published");
    assert!(bridge.status().available);
    assert_eq!(bridge.status().backend, "web_cookie");
    runtime.shutdown().await;

    let product = std::fs::read_to_string(&config_path).unwrap();
    assert!(product.contains("cookie_secret_key = \"BILIBILI_COOKIE\""));
    assert!(!product.contains("SESSDATA"));
}

#[tokio::test]
async fn bilibili_fails_startup_without_image_renderer_protocol() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let secret_path = root.path().join("product.secret.toml");
    std::fs::write(&secret_path, "[secrets]\n").unwrap();
    let secret_path = secret_path.to_string_lossy().replace('\\', "/");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[security]
secret_file = "{secret_path}"

[[plugins.configured]]
id = "mutsuki.std.resource.memory"

[[plugins.configured]]
id = "mutsuki.bot.command"

[plugins.configured.config]
prefixes = ["/"]

[[plugins.configured]]
id = "mutsuki.bot.bilibili"

[plugins.configured.config]
backend = {{ type = "web_cookie", cookie_secret_key = "BILIBILI_COOKIE" }}
live_interval_ms = 60000
dynamic_interval_ms = 60000
video_interval_ms = 60000
retry = {{ max_attempts = 3, initial_backoff_ms = 100, max_backoff_ms = 1000 }}
subscriptions = []
link_resolver = {{ enabled = false, cooldown_ms = 1000, account_to_binding = {{}} }}
media_provider_id = "mutsuki.std.resource.memory"
management = {{ enabled = true, allow_self_binding = true, command = "bili", admin_user_ids = ["admin"], self_binding_notifications = ["dynamic"], self_binding_outbound_binding = "qq-main" }}
"#,
            ),
        ),
    )
    .unwrap();

    let error = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("missing image renderer protocol must fail startup");
    assert!(error.to_string().contains("mutsuki.image.card.render"));
}

#[tokio::test]
async fn workshop_fails_startup_without_explicit_media_provider() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            r#"
[[plugins.configured]]
id = "mutsuki.bot.bilibili.workshop"

[plugins.configured.config]
media_provider_id = "missing.media.provider"
"#,
        ),
    )
    .unwrap();
    let error = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("missing provider must fail startup");
    assert!(error.to_string().contains("missing.media.provider"));
}

#[tokio::test]
async fn mihuashi_fails_startup_without_image_renderer_protocol() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            r#"
[[plugins.configured]]
id = "mutsuki.std.resource.memory"

[[plugins.configured]]
id = "template.test.browser"

[[plugins.configured]]
id = "mutsuki.std.io.http_client"

[plugins.configured.config]
response_provider_id = "mutsuki.std.resource.memory"
domain_allowlist = ["mihuashi.com"]

[[plugins.configured]]
id = "mutsuki.bot.mihuashi"

[plugins.configured.config]
media_provider_id = "mutsuki.std.resource.memory"
"#,
        ),
    )
    .unwrap();
    let error = assemble_with_browser_stub(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("missing image renderer protocol must fail startup");
    assert!(
        error.to_string().contains("mutsuki.image.card.render"),
        "{error}"
    );
}

#[tokio::test]
async fn mihuashi_with_renderer_fails_startup_without_browser_protocol() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let font = test_font_path();
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[[plugins.configured]]
id = "mutsuki.std.resource.memory"

[[plugins.configured]]
id = "mutsuki.std.image.render"

[plugins.configured.config]
output_provider_id = "mutsuki.std.resource.memory"
font_files = ["{font}"]

[[plugins.configured]]
id = "mutsuki.std.io.http_client"

[plugins.configured.config]
response_provider_id = "mutsuki.std.resource.memory"
domain_allowlist = ["mihuashi.com"]

[[plugins.configured]]
id = "mutsuki.bot.mihuashi"

[plugins.configured.config]
media_provider_id = "mutsuki.std.resource.memory"
"#,
            ),
        ),
    )
    .unwrap();
    let error = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("missing browser protocol must fail startup");
    assert!(
        error.to_string().contains("mutsuki.browser.snapshot"),
        "{error}"
    );
}

#[tokio::test]
async fn mihuashi_browser_renderer_and_provider_form_valid_load_plan() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let font = test_font_path();
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[[plugins.configured]]
id = "mutsuki.std.resource.memory"

[[plugins.configured]]
id = "template.test.browser"

[[plugins.configured]]
id = "mutsuki.std.io.http_client"

[plugins.configured.config]
response_provider_id = "mutsuki.std.resource.memory"
domain_allowlist = ["mihuashi.com"]

[[plugins.configured]]
id = "mutsuki.std.image.render"

[plugins.configured.config]
output_provider_id = "mutsuki.std.resource.memory"
font_files = ["{font}"]

[[plugins.configured]]
id = "mutsuki.bot.mihuashi"

[plugins.configured.config]
media_provider_id = "mutsuki.std.resource.memory"
"#,
            ),
        ),
    )
    .unwrap();

    let runtime = assemble_with_browser_stub(load(&config_path))
        .unwrap()
        .start()
        .await
        .unwrap();
    runtime.shutdown().await;
}

#[tokio::test]
async fn image_renderer_rejects_missing_font_before_startup() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            r#"
[[plugins.configured]]
id = "mutsuki.std.resource.memory"

[[plugins.configured]]
id = "mutsuki.std.image.render"

[plugins.configured.config]
output_provider_id = "mutsuki.std.resource.memory"
font_files = ["/definitely/missing/NotoSansSC.ttf"]
"#,
        ),
    )
    .unwrap();
    let error = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("missing font must fail startup");
    assert!(error.to_string().contains("font file"));
}

#[tokio::test]
async fn image_renderer_fails_startup_without_output_provider() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let font = test_font_path();
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[[plugins.configured]]
id = "mutsuki.std.image.render"

[plugins.configured.config]
output_provider_id = "missing.output.provider"
font_files = ["{font}"]
"#,
            ),
        ),
    )
    .unwrap();
    let error = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("missing output provider must fail startup");
    assert!(
        error.to_string().contains("missing.output.provider"),
        "{error}"
    );
}

#[tokio::test]
async fn bilibili_chromium_backend_fails_startup_without_browser_protocol() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let secret_path = root.path().join("product.secret.toml");
    std::fs::write(&secret_path, "[secrets]\n").unwrap();
    let secret_path = secret_path.to_string_lossy().replace('\\', "/");
    let font = test_font_path();
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &format!(
                r#"
[security]
secret_file = "{secret_path}"

[[plugins.configured]]
id = "mutsuki.std.resource.memory"

[[plugins.configured]]
id = "mutsuki.std.image.render"

[plugins.configured.config]
output_provider_id = "mutsuki.std.resource.memory"
font_files = ["{font}"]

[[plugins.configured]]
id = "mutsuki.bot.bilibili"

[plugins.configured.config]
backend = {{ type = "web_cookie", cookie_secret_key = "BILIBILI_COOKIE" }}
live_interval_ms = 60000
dynamic_interval_ms = 60000
video_interval_ms = 60000
retry = {{ max_attempts = 3, initial_backoff_ms = 100, max_backoff_ms = 1000 }}
subscriptions = []
link_resolver = {{ enabled = false, cooldown_ms = 1000, account_to_binding = {{}} }}
media_provider_id = "mutsuki.std.resource.memory"
risk_control = {{ backend = "chromium", timeout_ms = 10000, max_response_bytes = 2097152 }}
management = {{ enabled = true, allow_self_binding = true, command = "bili", admin_user_ids = ["admin"], self_binding_notifications = ["dynamic"], self_binding_outbound_binding = "qq-main" }}
"#,
            ),
        ),
    )
    .unwrap();
    let error = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("missing browser protocol must fail startup");
    assert!(error.to_string().contains("mutsuki.browser.snapshot"));
}

#[tokio::test]
async fn chromium_factory_rejects_missing_artifact_during_assembly() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            r#"
[[plugins.configured]]
id = "mutsuki.std.io.browser.chromium"

[plugins.configured.config]
executable = "/definitely/missing/chromium"
domain_allowlist = ["mihuashi.com"]
timeout_ms = 10000
max_dom_bytes = 2097152
"#,
        ),
    )
    .unwrap();
    let error = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("missing Chromium artifact must fail startup");
    assert!(error.to_string().contains("Chromium executable"));
}

#[tokio::test]
async fn bilibili_open_platform_rejects_web_only_dynamic_capability() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let secret_path = root.path().join("product.secret.toml");
    std::fs::write(
        &secret_path,
        r#"[secrets]
BILIBILI_OPEN_APP_SECRET = "TEST_APP_SECRET"
BILIBILI_OPEN_OAUTH = '''{"access_token":"TEST_ACCESS_TOKEN","refresh_token":"TEST_REFRESH_TOKEN","expires_at":1893456000,"scopes":["LIVE_ROOM_DATA","ARC_BASE"]}'''
"#,
    )
    .unwrap();
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &bilibili_open_platform_config(&secret_path, true),
        ),
    )
    .unwrap();

    let error = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .err()
        .expect("Open Platform must reject dynamic polling instead of falling back");
    assert!(error.to_string().contains("poll/dynamic"));
}

#[tokio::test]
async fn bilibili_open_platform_fails_preflight_without_each_host_secret() {
    for (secret_body, missing_key) in [
        ("[secrets]\n", "BILIBILI_OPEN_APP_SECRET"),
        (
            "[secrets]\nBILIBILI_OPEN_APP_SECRET = \"TEST_APP_SECRET\"\n",
            "BILIBILI_OPEN_OAUTH",
        ),
    ] {
        let root = tempdir().unwrap();
        let config_path = root.path().join("product.toml");
        let secret_path = root.path().join("product.secret.toml");
        std::fs::write(&secret_path, secret_body).unwrap();
        std::fs::write(
            &config_path,
            service_toml(
                root.path(),
                &bilibili_open_platform_config(&secret_path, false),
            ),
        )
        .unwrap();

        let error = assemble_service(load(&config_path))
            .unwrap()
            .start()
            .await
            .err()
            .expect("Open Platform must fail before runtime without every Host secret");
        assert!(error.to_string().contains(missing_key));
    }
}

#[tokio::test]
async fn bilibili_open_platform_idle_runtime_starts_without_web_fallback() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("product.toml");
    let secret_path = root.path().join("product.secret.toml");
    std::fs::write(
        &secret_path,
        r#"[secrets]
BILIBILI_OPEN_APP_SECRET = "TEST_APP_SECRET"
BILIBILI_OPEN_OAUTH = '''{"access_token":"TEST_ACCESS_TOKEN","refresh_token":"TEST_REFRESH_TOKEN","expires_at":1893456000,"scopes":["LIVE_ROOM_DATA","ARC_BASE"]}'''
"#,
    )
    .unwrap();
    std::fs::write(
        &config_path,
        service_toml(
            root.path(),
            &bilibili_open_platform_config(&secret_path, false),
        ),
    )
    .unwrap();

    let runtime = assemble_service(load(&config_path))
        .unwrap()
        .start()
        .await
        .unwrap();
    runtime.shutdown().await;

    let product = std::fs::read_to_string(&config_path).unwrap();
    assert!(product.contains("type = \"open_platform\""));
    assert!(!product.contains("TEST_APP_SECRET"));
    assert!(!product.contains("TEST_ACCESS_TOKEN"));
    assert!(!product.contains("cookie_secret_key"));
}

#[test]
fn committed_template_exposes_only_product_configuration() {
    let root = tempdir().unwrap();
    let config_path = root.path().join("local.toml");
    let secret_path = root.path().join("local.secret.toml");
    let home = root
        .path()
        .join("home")
        .to_string_lossy()
        .replace('\\', "/");
    let config = SIMPLE_TEMPLATE.replace("[service]", &format!("[service]\nhome_dir = \"{home}\""));
    std::fs::write(&config_path, config).unwrap();
    std::fs::write(&secret_path, "[secrets]\n").unwrap();

    let service = load(&config_path);

    assert_eq!(service.service.instance_id, "mutsuki-bot");
    assert!(service.plugins.configured.is_empty());
    assert_eq!(service.core.max_tasks, 4096);
    assert!(service.runners.restart);
    assert!(!SIMPLE_TEMPLATE.contains("[core]"));
    assert!(!SIMPLE_TEMPLATE.contains("[runners]"));
    assert!(!SIMPLE_TEMPLATE.contains("[observe]"));
}

fn load(path: &Path) -> ServiceConfig {
    ServiceConfig::load(ConfigOverrides {
        config_file: Some(path.to_path_buf()),
        ..Default::default()
    })
    .unwrap()
}

fn test_font_path() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fonts/NotoSansSC-Test.ttf")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

struct BrowserStubRunner {
    descriptor: RunnerDescriptor,
}

impl Runner for BrowserStubRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        Ok(CompletionBatch::from_error(
            &batch,
            RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                "template.test.browser",
                "browser.stub.must_not_run",
            ),
        ))
    }
}

fn browser_stub_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new("template.test.browser.runner", "template.test.browser")
        .accepted_protocol(mutsuki_protocol_browser::SNAPSHOT)
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Blocking)
        .build()
}

fn browser_stub_manifest() -> PluginManifest {
    let mut manifest = PluginBuilder::new("template.test.browser")
        .runner(Box::new(BrowserStubRunner {
            descriptor: browser_stub_descriptor(),
        }))
        .protocol_handler(
            ProtocolDescriptorBuilder::new(mutsuki_protocol_browser::SNAPSHOT)
                .input_schema(
                    mutsuki_protocol_browser::input_schema(mutsuki_protocol_browser::SNAPSHOT)
                        .unwrap(),
                )
                .output_schema(
                    mutsuki_protocol_browser::output_schema(mutsuki_protocol_browser::SNAPSHOT)
                        .unwrap(),
                )
                .error_schema(
                    mutsuki_protocol_browser::error_schema(mutsuki_protocol_browser::SNAPSHOT)
                        .unwrap(),
                )
                .build(),
            "template.test.browser.runner",
            "blocking",
        )
        .build()
        .manifest;
    manifest.provides.protocol_classes.insert(
        mutsuki_protocol_browser::SNAPSHOT.into(),
        ProtocolClass::Effect,
    );
    manifest
}

struct BrowserStubConfiguredPlugin;

impl ConfiguredPluginFactory for BrowserStubConfiguredPlugin {
    fn plugin_id(&self) -> &str {
        "template.test.browser"
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        if !config.is_null() && config.as_object().is_none_or(|object| !object.is_empty()) {
            return Err("browser stub does not accept configuration".into());
        }
        Ok(builder
            .register_builtin_plugin(browser_stub_manifest())
            .register_builtin_runner(|| {
                Box::new(BrowserStubRunner {
                    descriptor: browser_stub_descriptor(),
                })
            }))
    }
}

fn assemble_with_browser_stub(
    service: ServiceConfig,
) -> ServiceRuntimeResult<ServiceRuntimeBuilder> {
    let mut catalog = configured_std_plugin_catalog()?;
    catalog.merge(configured_bot_plugin_catalog()?)?;
    catalog.register(BrowserStubConfiguredPlugin)?;
    Ok(ServiceRuntimeBuilder::new(service).with_configured_plugin_catalog(catalog))
}

fn service_toml(root: &Path, configured: &str) -> String {
    format!(
        r#"[service]
profile = "test"
instance_id = "template-test"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "named-pipe"
name = "template-test"
token = "test-token"

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"
{}

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
        root.to_string_lossy().replace('\\', "/"),
        configured
    )
}

fn bilibili_open_platform_config(secret_path: &Path, dynamic: bool) -> String {
    let secret_path = secret_path.to_string_lossy().replace('\\', "/");
    let font = test_font_path();
    let subscriptions = if dynamic {
        r#"[[plugins.configured.config.subscriptions]]
subscription_id = "invalid-dynamic"
uid = 42
notifications = ["dynamic"]
target = { type = "group", group_id = "test-group" }
outbound_binding = "qq-main"
paused = false
"#
    } else {
        "subscriptions = []"
    };
    format!(
        r#"[security]
secret_file = "{secret_path}"

[[plugins.configured]]
id = "mutsuki.std.resource.memory"

[[plugins.configured]]
id = "mutsuki.std.image.render"

[plugins.configured.config]
output_provider_id = "mutsuki.std.resource.memory"
font_files = ["{font}"]

[[plugins.configured]]
id = "mutsuki.bot.bilibili"

[plugins.configured.config]
backend = {{ type = "open_platform", client_id = "test-client", app_secret_key = "BILIBILI_OPEN_APP_SECRET", oauth_credential_key = "BILIBILI_OPEN_OAUTH", authorized_uid = 42 }}
live_interval_ms = 60000
dynamic_interval_ms = 60000
video_interval_ms = 60000
retry = {{ max_attempts = 3, initial_backoff_ms = 100, max_backoff_ms = 1000 }}
link_resolver = {{ enabled = false, cooldown_ms = 1000, account_to_binding = {{}} }}
media_provider_id = "mutsuki.std.resource.memory"
management = {{ enabled = false, allow_self_binding = false, command = "bili", admin_user_ids = [], self_binding_notifications = ["live", "video"], self_binding_outbound_binding = "" }}
{subscriptions}
"#,
    )
}
