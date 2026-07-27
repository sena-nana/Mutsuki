use std::path::{Path, PathBuf};

use mutsuki_bot_service_host_integration::configured_bot_plugin_catalog;
use mutsuki_service_config::{ExecutionClassName, ExecutionDomainSection, ServiceConfig};
use mutsuki_service_runtime::{ServiceRuntimeBuilder, ServiceRuntimeResult};
use mutsuki_std_plugins::configured_std_plugin_catalog;

mod distribution;
mod web_console;
pub use distribution::*;
pub use web_console::*;

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
pub fn apply_product_runtime_profile(service: &mut ServiceConfig) {
    if service.service.profile != "bot" || !service.core.execution_domains.is_empty() {
        return;
    }
    service.core.execution_domains = vec![
        execution_domain("bot-control", vec![ExecutionClassName::Orchestration], 2),
        execution_domain("network-io", vec![ExecutionClassName::Io], 4),
        execution_domain("blocking-adapters", vec![ExecutionClassName::Blocking], 2),
        execution_domain(
            "agent-compute",
            vec![ExecutionClassName::Cpu, ExecutionClassName::Script],
            2,
        ),
    ];
}

fn execution_domain(
    id: &str,
    execution_classes: Vec<ExecutionClassName>,
    threads: usize,
) -> ExecutionDomainSection {
    ExecutionDomainSection {
        id: id.into(),
        execution_classes,
        threads,
        ..ExecutionDomainSection::default()
    }
}

pub fn assemble_service(mut service: ServiceConfig) -> ServiceRuntimeResult<ServiceRuntimeBuilder> {
    apply_product_runtime_profile(&mut service);
    let mut catalog = configured_std_plugin_catalog()?;
    catalog.merge(configured_bot_plugin_catalog()?)?;
    Ok(ServiceRuntimeBuilder::new(service).with_configured_plugin_catalog(catalog))
}
