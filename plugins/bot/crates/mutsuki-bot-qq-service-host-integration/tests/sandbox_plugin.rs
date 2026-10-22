mod support;

use std::time::{Duration, Instant};

use mutsuki_bot_flow::{BOT_FLOW_CONFIG_PROVIDER_ID, BotFlowConfigProvider};
use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_FLOW_BOT_EVENT_TYPE, BotConversationKind, BotFlowDocument,
    BotFlowEdge, BotFlowEdgeKind, BotFlowNode, BotFlowNodePosition, BotFlowSourceSelector,
    BotFlowTypeRef, BotPlatform,
};
use mutsuki_bot_sandbox::{
    SANDBOX_SERVICE_ID, SandboxAction, SandboxApi, SandboxMode, SandboxService, SandboxSpeakerRole,
    SandboxWriteRequest, sandbox_user_id,
};
use mutsuki_bot_service_host_integration::BOT_SANDBOX_PLUGIN_ID;
use mutsuki_bot_testkit::{FakeQqGatewayScript, FakeQqServer};
use mutsuki_config_service::{ConfigApplyRequest, ConfigContext, ConfigRevision, capability};
use mutsuki_plugin_bot_adapter_qqbot::QQBOT_ADAPTER_PLUGIN_ID;
use mutsuki_plugin_bot_event_router::{BOT_FLOW_ROUTER_PLUGIN_ID, BOT_SEND_NODE_TYPE};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qq_alone_publishes_no_sandbox() {
    let fake = FakeQqServer::start_with_gateway_script(FakeQqGatewayScript {
        close_delay: Duration::from_secs(2),
        ..FakeQqGatewayScript::default()
    })
    .await;
    let (config, _root) = qq_service_config(&fake, false);
    let runtime = {
        let mut catalog =
            mutsuki_std_service_host_integration::configured_std_plugin_catalog().unwrap();
        catalog
            .merge(support::bot_catalog_with_qq_and_flow().0)
            .unwrap();
        ServiceRuntimeBuilder::new(config)
            .with_configured_plugin_catalog(catalog)
            .start()
            .await
            .unwrap()
    };
    // The sandbox is its own adapter; QQ neither wraps nor publishes it.
    assert!(
        runtime
            .host_service::<SandboxService>(SANDBOX_SERVICE_ID)
            .is_err()
    );
    runtime.shutdown().await;
    fake.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_live_mode_observes_qq_through_flow_ingress() {
    let fake = FakeQqServer::start_with_gateway_script(FakeQqGatewayScript {
        initial_events: vec![json!({
            "op": 0,
            "s": 2,
            "t": "GROUP_AT_MESSAGE_CREATE",
            "id": "group-title-event",
            "d": {
                "id": "group-title-message",
                "group_openid": "GROUP_1",
                "content": "<@BOT_OPENID> ping",
                "mentions": [{"id": "BOT_OPENID", "is_you": true, "bot": true}],
                "timestamp": "2026-07-12T10:00:00+08:00",
                "author": {"member_openid": "USER_1", "username": "tester"}
            }
        })],
        resumed_events: Vec::new(),
        close_delay: Duration::from_secs(2),
        ..FakeQqGatewayScript::default()
    })
    .await;
    let (config, _root) = qq_service_config(&fake, true);
    let runtime = {
        let mut catalog =
            mutsuki_std_service_host_integration::configured_std_plugin_catalog().unwrap();
        catalog
            .merge(support::bot_catalog_with_qq_and_flow().0)
            .unwrap();
        ServiceRuntimeBuilder::new(config)
            .with_configured_plugin_catalog(catalog)
            .start()
            .await
            .expect("sandbox adapter plus QQ must boot")
    };
    let sandbox = runtime
        .host_service::<SandboxService>(SANDBOX_SERVICE_ID)
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut title = None;
    while Instant::now() < deadline {
        let snapshot = sandbox.snapshot("").await.unwrap();
        if snapshot.mode != SandboxMode::Live {
            let _ = sandbox
                .write(
                    "test",
                    SandboxWriteRequest {
                        operation_id: format!("live-{}", snapshot.revision),
                        expected_revision: snapshot.revision,
                        action: SandboxAction::SetMode {
                            mode: SandboxMode::Live,
                        },
                    },
                )
                .await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }
        title = snapshot
            .conversations
            .iter()
            .find(|item| item.kind == BotConversationKind::Group)
            .map(|item| item.title.clone());
        if title.as_deref() == Some("测试群") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    runtime.shutdown().await;
    fake.shutdown().await;
    assert_eq!(title.as_deref(), Some("测试群"));
}

fn qq_service_config(
    fake: &FakeQqServer,
    select_workspace_sandbox: bool,
) -> (ServiceConfig, tempfile::TempDir) {
    qq_service_config_ordered(fake, select_workspace_sandbox, false)
}

/// `sandbox_after_qq` selects the sandbox after the QQ adapter: preparation order must not
/// decide whether the two share one simulator.
fn qq_service_config_ordered(
    fake: &FakeQqServer,
    select_workspace_sandbox: bool,
    sandbox_after_qq: bool,
) -> (ServiceConfig, tempfile::TempDir) {
    let secret_key = format!(
        "QQBOT_SANDBOX_PLUGIN_SECRET_{}",
        fake.websocket_addr().port()
    );
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("local.secret.toml"),
        format!("[secrets]\n{secret_key} = \"TEST_CLIENT_SECRET\"\n"),
    )
    .unwrap();
    let qq = fake.config("sandbox-plugin", "TEST_APP_ID", &secret_key);
    let config_path = root.path().join("local.toml");
    let sandbox_block = if select_workspace_sandbox {
        format!(
            r#"
[[plugins.configured]]
id = "{BOT_FLOW_ROUTER_PLUGIN_ID}"
enabled = true

[[plugins.configured]]
id = "{BOT_SANDBOX_PLUGIN_ID}"
enabled = true
"#
        )
    } else {
        String::new()
    };
    let (sandbox_selection, sandbox_tail) = if sandbox_after_qq {
        (String::new(), sandbox_block)
    } else {
        (sandbox_block, String::new())
    };
    std::fs::write(
        &config_path,
        format!(
            r#"[service]
profile = "sandbox-plugin"
instance_id = "sandbox-plugin-{}"
home_dir = "{}"
data_dir = "data"
log_dir = "logs"
plugin_dir = "plugins"
run_dir = "run"

[ipc]
enabled = false
transport = "tcp-debug"
name = "sandbox-plugin"
token = "test-token"

[plugins]
dynamic_dirs = []
disabled_dir = "disabled"
{sandbox_selection}
[[plugins.configured]]
id = "mutsuki.std.resource.sqlite"

[plugins.configured.config]
database_path = "{}"

[[plugins.configured]]
id = "{QQBOT_ADAPTER_PLUGIN_ID}"
[plugins.configured.config]
account_id = "{}"
app_id = "{}"
client_secret_key = "{}"
token_url = "{}"
openapi_base_url = "{}"
allow_insecure_transport = true
gateway_hello_timeout_ms = 1000
gateway_ack_timeout_ms = 500
retry_base_delay_ms = 0
retry_max_delay_ms = 0
reconnect_initial_delay_ms = 10
reconnect_max_delay_ms = 20
reconnect_jitter_ms = 0
{sandbox_tail}
[security]
secret_file = "local.secret.toml"

[observe]
console = false
json = false
log_file = "service.log"
panic_file = "panic.log"
"#,
            qq.client_secret_key,
            root.path().to_string_lossy().replace('\\', "/"),
            root.path()
                .join("resources.sqlite")
                .to_string_lossy()
                .replace('\\', "/"),
            qq.account_id,
            qq.app_id,
            qq.client_secret_key,
            qq.token_url,
            qq.openapi_base_url,
        ),
    )
    .unwrap();
    let config = ServiceConfig::load(ConfigOverrides {
        config_file: Some(config_path),
        ..Default::default()
    })
    .unwrap();
    (config, root)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_selected_after_qq_shares_the_simulator_and_reconfigures_repeatedly() {
    let fake = FakeQqServer::start_with_gateway_script(FakeQqGatewayScript {
        close_delay: Duration::from_secs(2),
        ..FakeQqGatewayScript::default()
    })
    .await;
    let (config, _root) = qq_service_config_ordered(&fake, true, true);
    let runtime = {
        let mut catalog =
            mutsuki_std_service_host_integration::configured_std_plugin_catalog().unwrap();
        catalog
            .merge(support::bot_catalog_with_qq_and_flow().0)
            .unwrap();
        ServiceRuntimeBuilder::new(config)
            .with_configured_plugin_catalog(catalog)
            .start()
            .await
            .expect("sandbox selected after QQ must boot")
    };
    let before = runtime
        .host_service::<SandboxService>(SANDBOX_SERVICE_ID)
        .unwrap();
    for _ in 0..2 {
        runtime
            .reconfigure_plugins(&[mutsuki_service_config::ConfiguredPluginSelection {
                id: BOT_SANDBOX_PLUGIN_ID.into(),
                enabled: true,
                config: json!({}),
            }])
            .await
            .expect("sandbox reconfigures repeatedly");
    }
    let after = runtime
        .host_service::<SandboxService>(SANDBOX_SERVICE_ID)
        .unwrap();
    // The simulator is application-lifetime state: reconfiguring the plugin keeps its history.
    assert!(std::sync::Arc::ptr_eq(&before, &after));
    runtime.shutdown().await;
    fake.shutdown().await;
}

/// One neutral graph (any platform's message -> `mutsuki.bot.send`) serves QQ and the sandbox at
/// once: both adapters load side by side, and each reply leaves through the account its message
/// arrived on -- QQ's through the QQ endpoint, the sandbox's as a simulated bot bubble.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qq_and_sandbox_replies_route_by_account() {
    let fake = FakeQqServer::start_with_gateway_script(FakeQqGatewayScript {
        initial_events: Vec::new(),
        // Delivered after the Gateway resumes, by which time the graph below is live.
        resumed_events: vec![json!({
            "op": 0,
            "s": 4,
            "t": "GROUP_AT_MESSAGE_CREATE",
            "id": "qq-ping",
            "d": {
                "id": "QQ_MESSAGE_PING",
                "group_openid": "GROUP_1",
                "content": "<@BOT_OPENID> ping",
                "mentions": [{"id": "BOT_OPENID", "is_you": true, "bot": true}],
                "timestamp": "2026-07-12T10:00:00+08:00",
                "author": {"member_openid": "USER_1", "username": "tester"}
            }
        })],
        close_delay: Duration::from_secs(2),
        ..FakeQqGatewayScript::default()
    })
    .await;
    let (config, _root) = qq_service_config(&fake, true);
    let (catalog, flow_config) = support::bot_catalog_with_qq_and_flow();
    let runtime = {
        let mut merged =
            mutsuki_std_service_host_integration::configured_std_plugin_catalog().unwrap();
        merged.merge(catalog).unwrap();
        ServiceRuntimeBuilder::new(config)
            .with_configured_plugin_catalog(merged)
            .start()
            .await
            .expect("QQ and the sandbox adapter load together")
    };
    flow_config
        .apply(
            BOT_FLOW_CONFIG_PROVIDER_ID,
            ConfigApplyRequest {
                candidate: BotFlowConfigProvider::encode(&reply_flow()).unwrap(),
                expected_revision: ConfigRevision(0),
                dry_run: false,
            },
            ConfigContext::global(),
            &[capability::VALUE_WRITE.into(), capability::APPLY.into()],
        )
        .await
        .expect("neutral reply graph applies");

    let sandbox = runtime
        .host_service::<SandboxService>(SANDBOX_SERVICE_ID)
        .unwrap();
    let snapshot = sandbox.snapshot("").await.unwrap();
    assert!(snapshot.flow_available);
    let group = snapshot
        .conversations
        .iter()
        .find(|item| item.kind == BotConversationKind::Group)
        .expect("seeded sandbox group")
        .clone();
    assert_eq!(group.conversation.platform, BotPlatform::Sandbox);
    sandbox
        .write(
            "tester",
            SandboxWriteRequest {
                operation_id: "sandbox-ping".into(),
                expected_revision: snapshot.revision,
                action: SandboxAction::IngestAsUser {
                    conversation_id: group.conversation_id.clone(),
                    user_id: sandbox_user_id("Alice"),
                    text: "ping".into(),
                    segments: Vec::new(),
                    reply_to: None,
                },
            },
        )
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bubble = None;
    while Instant::now() < deadline && bubble.is_none() {
        bubble = sandbox
            .messages(&group.conversation_id)
            .await
            .unwrap()
            .into_iter()
            .find(|item| item.role == SandboxSpeakerRole::Bot && item.text == "pong");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(bubble.is_some(), "sandbox reply recorded as a bot bubble");

    let sends = fake.wait_for_sends(1, Duration::from_secs(10)).await;
    assert_eq!(
        sends.len(),
        1,
        "only the QQ message is answered on QQ: {sends:?}"
    );
    assert_eq!(sends[0]["content"], "pong", "{sends:?}");
    assert_eq!(
        sends[0]["msg_id"], "QQ_MESSAGE_PING",
        "a passive reply to the QQ message"
    );
    runtime.shutdown().await;
    fake.shutdown().await;
}

fn reply_flow() -> BotFlowDocument {
    BotFlowDocument {
        flow_id: "neutral.reply".into(),
        name: "neutral reply".into(),
        nodes: vec![
            BotFlowNode {
                node_id: "source".into(),
                node_type_id: "mutsuki.bot.message.created".into(),
                node_type_version: 1,
                config: json!({}),
                source: Some(BotFlowSourceSelector {
                    protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
                    event_type: Some(BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1)),
                }),
                position: BotFlowNodePosition::default(),
            },
            BotFlowNode {
                node_id: "send".into(),
                node_type_id: BOT_SEND_NODE_TYPE.into(),
                node_type_version: 1,
                config: json!({"text": "pong", "reply": true}),
                source: None,
                position: BotFlowNodePosition::default(),
            },
        ],
        edges: vec![BotFlowEdge {
            edge_id: "source-send".into(),
            from_node_id: "source".into(),
            from_port_id: "event".into(),
            to_node_id: "send".into(),
            to_port_id: "event".into(),
            kind: BotFlowEdgeKind::Event,
        }],
    }
}
