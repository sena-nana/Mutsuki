use std::sync::Arc;

use mutsuki_bot_management::{PlatformConsoleProvider, QqPlatformConsoleProvider};
use mutsuki_bot_protocol::{
    BotAdapterCapabilities, BotConversationKind, BotRateLimitPolicy, BotSegmentKind,
    BotStreamingStrategy, BotUploadConstraints,
};
use mutsuki_bot_qq_protocol::{QqBotCapabilityMatrix, QqPermissionRequirement};
use mutsuki_bot_sandbox::{SandboxApi, SandboxService};
use mutsuki_bot_web_extension_qq::{
    LocalQqManagementProvider, QqAccountViewInput, QqBotManagementApi, QqBotManagementService,
    account_view_from_config,
};
use mutsuki_bot_web_host_integration::{
    BotConsoleServices, ConsoleActivity, ConsoleLayout, WebConsoleConfig, WebConsolePaths,
    WebConsoleSecrets, attach_control_changed_bridge, build_bot_console, demo_config_service,
};
use mutsuki_service_control::{ControlChangeDomain, ControlChangeEvent, ControlChangeSubscription};
use mutsuki_web_extension_control::FixtureControlHandler;
use mutsuki_web_host::WebHost;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The Playwright smoke pins the address; manual runs take any free port.
    let listen =
        std::env::var("MUTSUKI_CONSOLE_FIXTURE_LISTEN").unwrap_or_else(|_| "127.0.0.1:0".into());
    let mut layout = ConsoleLayout::default();
    layout.activities.extend([
        ConsoleActivity::top("platforms", "平台与消息", "bot", 10),
        ConsoleActivity::top("sandbox", "沙盒", "sandbox", 30),
    ]);
    let config = WebConsoleConfig {
        enabled: true,
        listen,
        extensions: vec!["config".into(), "sandbox".into(), "qq".into()],
        layout,
        ..Default::default()
    };
    let qq = fixture_qq();
    let platforms_qq = qq.clone();
    let secrets = WebConsoleSecrets {
        auth_token: "embedded-console-fixture".into(),
    };
    let control = FixtureControlHandler::default();
    let uptime_ms = control.uptime_ms.clone();
    let service_status_calls = control.service_status_calls.clone();
    let (mut host, _assets) = build_bot_console(
        &config,
        &secrets,
        Arc::new(control),
        "fixture",
        &WebConsolePaths::default(),
        BotConsoleServices {
            config: Some(demo_config_service()),
            sandbox: Some(Arc::new(SandboxService::new()) as Arc<dyn SandboxApi>),
            qq: Some(qq),
            // The real QQ projection over a local (in-memory) management service.
            platforms: Some(Arc::new(move || {
                vec![Arc::new(QqPlatformConsoleProvider::new(
                    "mutsuki.plugin.bot.adapter-qqbot",
                    platforms_qq.clone(),
                )) as Arc<dyn PlatformConsoleProvider>]
            })),
            ..BotConsoleServices::default()
        },
    )?;
    host.start().await?;
    let (changes, receiver) = tokio::sync::broadcast::channel(8);
    let _changes = attach_control_changed_bridge(&host, ControlChangeSubscription::new(receiver))
        .ok_or("fixture has no event bridge")?;
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        uptime_ms.fetch_add(60_000, std::sync::atomic::Ordering::AcqRel);
        let _ = changes.send(ControlChangeEvent {
            revision: 1,
            domains: vec![ControlChangeDomain::Tasks],
        });
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        println!(
            "overview_snapshot_reads={}",
            service_status_calls.load(std::sync::atomic::Ordering::Acquire)
        );
    });
    let address = host.listen_addr().ok_or("fixture has no listen address")?;
    println!("http://{address}");
    std::future::pending::<()>().await;
    #[allow(unreachable_code)]
    host.stop().await?;
    Ok(())
}

/// One identified QQ account held in memory; no QQ endpoint or credential is involved.
fn fixture_qq() -> Arc<dyn QqBotManagementApi> {
    let local = LocalQqManagementProvider::new();
    local.upsert_account(account_view_from_config(QqAccountViewInput {
        account_id: "main".into(),
        app_id: "fixture-app".into(),
        credential_reference: "QQBOT_CLIENT_SECRET".into(),
        credential_present: true,
        capability: QqBotCapabilityMatrix {
            account_id: "main".into(),
            configured_intents: 1,
            shard: [0, 1],
            required_intents: vec!["group_and_c2c_event".into()],
            required_permissions: vec![QqPermissionRequirement::ReadC2cMessages],
            capabilities: BotAdapterCapabilities {
                conversation_kinds: vec![BotConversationKind::Private],
                outbound_conversation_kinds: vec![BotConversationKind::Private],
                active_message_kinds: vec![BotConversationKind::Private],
                inbound_segments: vec![BotSegmentKind::Text],
                outbound_segments: vec![BotSegmentKind::Text],
                inbound_media: Vec::new(),
                outbound_media: Vec::new(),
                active_message: true,
                message_edit: false,
                message_recall: true,
                reply: true,
                quote: true,
                mention: true,
                upload: BotUploadConstraints::default(),
                rate_limit: BotRateLimitPolicy::default(),
                streaming: vec![BotStreamingStrategy::FinalOnly],
            },
        },
        intents: 1,
        shard: [0, 1],
        connected: true,
        identified: true,
        last_heartbeat_unix_ms: Some(1),
        last_error: None,
        last_error_code: None,
        reconnect_count: 0,
        self_user: None,
    }));
    Arc::new(QqBotManagementService::local(local))
}
