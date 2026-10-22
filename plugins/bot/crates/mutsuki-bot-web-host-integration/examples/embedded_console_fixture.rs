use std::sync::Arc;

use mutsuki_bot_sandbox::{SandboxApi, SandboxService};
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
    layout
        .activities
        .push(ConsoleActivity::top("sandbox", "沙盒", "sandbox", 30));
    let config = WebConsoleConfig {
        enabled: true,
        listen,
        extensions: vec!["config".into(), "sandbox".into()],
        layout,
        ..Default::default()
    };
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
