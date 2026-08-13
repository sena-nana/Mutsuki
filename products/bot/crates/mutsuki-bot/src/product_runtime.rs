use std::sync::Arc;

use crate::{
    SingleInstanceProduct, TargetedPluginReloadLifecycle, WebConsoleError, WebConsoleGuard,
    assemble_service_with_connections,
};

#[derive(Debug, thiserror::Error)]
pub enum ProductRunError {
    #[error(transparent)]
    Service(#[from] mutsuki_service_runtime::ServiceRuntimeError),
    #[error(transparent)]
    ConsoleStart(#[from] WebConsoleError),
    #[error(transparent)]
    ConsoleStop(#[from] mutsuki_web_host::WebHostError),
}

pub async fn run_single_instance_product(
    product: SingleInstanceProduct,
) -> Result<(), ProductRunError> {
    let SingleInstanceProduct {
        service,
        config,
        console,
        root,
        agent_connections,
    } = product;
    let mut runtime =
        assemble_service_with_connections(service.clone(), config.clone(), agent_connections)?
            .start()
            .await?;
    config.set_lifecycle(Arc::new(TargetedPluginReloadLifecycle::new(
        runtime.handle(),
    )));
    let console = match WebConsoleGuard::start(console, &root, &service, &runtime, config).await {
        Ok(console) => console,
        Err(error) => {
            runtime.shutdown().await;
            return Err(ProductRunError::ConsoleStart(error));
        }
    };
    if let Some(console) = &console
        && let Some(addr) = console.listen_addr()
    {
        eprintln!("Mutsuki Web Console listening on http://{addr}");
    }

    let wait_result = runtime.wait_for_shutdown_request().await;
    let console_result = match console {
        Some(console) => console.stop().await,
        None => Ok(()),
    };
    runtime.shutdown().await;
    wait_result?;
    console_result.map_err(ProductRunError::ConsoleStop)
}
