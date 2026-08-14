use std::sync::Arc;

use mutsuki_bot_web_console::{
    WebConsoleConfig, WebConsolePaths, WebConsoleSecrets, attach_control_changed_bridge,
    build_console_host,
};
use mutsuki_plugin_bot_control_web::FixtureControlHandler;
use mutsuki_service_control::{ControlChangeDomain, ControlChangeEvent, ControlChangeSubscription};
use mutsuki_web_host::WebHost;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = WebConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:0".into(),
        ..Default::default()
    };
    let secrets = WebConsoleSecrets {
        auth_token: "embedded-console-fixture".into(),
    };
    let control = FixtureControlHandler::default();
    let uptime_ms = control.uptime_ms.clone();
    let service_status_calls = control.service_status_calls.clone();
    let (mut host, _assets) = build_console_host(
        &config,
        &secrets,
        Arc::new(control),
        "fixture",
        None,
        None,
        &WebConsolePaths::default(),
        None,
        None,
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
