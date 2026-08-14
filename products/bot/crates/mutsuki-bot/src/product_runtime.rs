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

#[derive(Debug, thiserror::Error)]
pub enum ProductEntryError {
    #[error(
        "single_instance.arguments_unsupported: mutsuki-bot accepts no configuration path or profile"
    )]
    ArgumentsUnsupported,
    #[error("{0}")]
    Load(String),
    #[error(transparent)]
    Run(#[from] ProductRunError),
}

/// Runs the canonical single-instance product entry used by the in-tree binary and generated
/// external product shells.
pub async fn run_single_instance_product_entry() -> Result<(), ProductEntryError> {
    reject_arguments(std::env::args_os().skip(1))?;
    let product = crate::load_single_instance_product()
        .await
        .map_err(ProductEntryError::Load)?;
    run_single_instance_product(product).await?;
    Ok(())
}

fn reject_arguments(
    arguments: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<(), ProductEntryError> {
    if arguments.into_iter().next().is_some() {
        Err(ProductEntryError::ArgumentsUnsupported)
    } else {
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_entry_rejects_configuration_arguments() {
        assert!(reject_arguments(Vec::new()).is_ok());
        assert!(matches!(
            reject_arguments(["local.toml".into()]),
            Err(ProductEntryError::ArgumentsUnsupported)
        ));
    }
}
