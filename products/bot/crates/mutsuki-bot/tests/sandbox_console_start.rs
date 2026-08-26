use mutsuki_bot::{
    WebConsoleGuard, assemble_service_with_connections, load_single_instance_product_for_test,
};
use mutsuki_plugin_bot_adapter_qqbot::QQBOT_ADAPTER_PLUGIN_ID;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_run_console_starts_without_qq() {
    let root = tempfile::tempdir().unwrap();
    let mut product = load_single_instance_product_for_test(root.path(), "test-admin-passphrase")
        .await
        .unwrap();
    assert!(
        product
            .service
            .plugins
            .configured
            .iter()
            .any(|selection| selection.id == QQBOT_ADAPTER_PLUGIN_ID && !selection.enabled)
    );
    product.console.listen = "127.0.0.1:0".into();

    let runtime = assemble_service_with_connections(
        product.service.clone(),
        product.config.clone(),
        product.agent_connections.clone(),
    )
    .unwrap()
    .start()
    .await
    .unwrap();
    let console = WebConsoleGuard::start(
        product.console,
        &product.root,
        &product.service,
        &runtime,
        product.config.clone(),
    )
    .await
    .expect("seeded console must start without QQ");
    assert!(console.is_some());
    if let Some(console) = console {
        console.stop().await.unwrap();
    }
    runtime.shutdown().await;
}
