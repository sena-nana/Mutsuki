use std::path::{Path, PathBuf};

use mutsuki_bot_service_host_integration::configured_bot_plugin_catalog;
use mutsuki_service_config::ServiceConfig;
use mutsuki_service_runtime::{ServiceRuntimeBuilder, ServiceRuntimeResult};
use mutsuki_std_plugins::configured_std_plugin_catalog;

mod distribution;
pub use distribution::*;

pub fn repository_local_config_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("mutsuki-bot crate must be inside the template workspace")
        .join("config")
        .join("local.toml")
}

/// Assemble a neutral ServiceRuntime from owner-provided plugin factories.
/// Configuration selects every platform, route, business plugin and provider.
pub fn assemble_service(service: ServiceConfig) -> ServiceRuntimeResult<ServiceRuntimeBuilder> {
    let mut catalog = configured_std_plugin_catalog()?;
    catalog.merge(configured_bot_plugin_catalog()?)?;
    Ok(ServiceRuntimeBuilder::new(service).with_configured_plugin_catalog(catalog))
}
