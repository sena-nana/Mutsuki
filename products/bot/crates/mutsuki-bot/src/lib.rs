use std::sync::Arc;

use mutsuki_agent_service_host_integration::{
    AgentConnectionRegistry, configured_standard_agent_plugin_catalog,
};
use mutsuki_bot_service_host_integration::configured_bot_plugin_catalog_with_agent;
use mutsuki_service_config::{ExecutionClassName, ExecutionDomainSection, ServiceConfig};
use mutsuki_service_runtime::{ServiceRuntimeBuilder, ServiceRuntimeResult};
use mutsuki_std_service_host_integration::configured_std_plugin_catalog;

mod bootstrap;
mod distribution;
mod lifecycle;
mod product_config;
#[cfg(feature = "web-console")]
mod web_console;
pub use bootstrap::*;
pub use distribution::*;
pub use lifecycle::*;
pub use product_config::*;
#[cfg(feature = "web-console")]
pub use web_console::*;

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

pub fn assemble_service(
    service: ServiceConfig,
    config: Arc<mutsuki_config_service::ConfigService>,
) -> ServiceRuntimeResult<ServiceRuntimeBuilder> {
    assemble_service_with_connections(service, config, AgentConnectionRegistry::new())
}

pub fn assemble_service_with_connections(
    mut service: ServiceConfig,
    config: Arc<mutsuki_config_service::ConfigService>,
    agent_connections: AgentConnectionRegistry,
) -> ServiceRuntimeResult<ServiceRuntimeBuilder> {
    apply_product_runtime_profile(&mut service);
    let mut catalog = configured_std_plugin_catalog()?;
    catalog.merge(configured_standard_agent_plugin_catalog(
        agent_connections.clone(),
        config.clone(),
    )?)?;
    catalog.merge(configured_bot_plugin_catalog_with_agent(
        config,
        agent_connections,
    )?)?;
    Ok(ServiceRuntimeBuilder::new(service).with_configured_plugin_catalog(catalog))
}
