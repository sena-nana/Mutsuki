// Each test binary compiles this module and uses only some of its helpers.
#![allow(dead_code)]

use std::sync::Arc;

use mutsuki_bot_qq_service_host_integration::configured_qq_plugin_catalog;
use mutsuki_bot_service_host_integration::{
    DEFAULT_MEDIA_PROVIDER_ID, configured_bot_plugin_catalog,
    configured_bot_plugin_catalog_with_config,
};
use mutsuki_config_service::{ConfigProviderRegistry, ConfigService, InMemoryConfigRepository};
use mutsuki_service_runtime::ConfiguredPluginCatalog;

/// Core Bot owners plus the QQ adapter, as a QQ product assembles them.
pub fn bot_catalog_with_qq() -> ConfiguredPluginCatalog {
    let mut catalog = configured_bot_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID.to_string()).unwrap();
    catalog
        .merge(configured_qq_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID).unwrap())
        .unwrap();
    catalog
}

/// Like [`bot_catalog_with_qq`] with the Flow Router available, which the sandbox adapter needs
/// to compose into ingress and to observe live adapters. Returns the `ConfigService` the Flow graph
/// is applied through.
pub fn bot_catalog_with_qq_and_flow() -> (ConfiguredPluginCatalog, Arc<ConfigService>) {
    let config = Arc::new(
        ConfigService::new(
            Arc::new(ConfigProviderRegistry::default()),
            Arc::new(InMemoryConfigRepository::default()),
        )
        .unwrap(),
    );
    let mut catalog = configured_bot_plugin_catalog_with_config(
        config.clone(),
        DEFAULT_MEDIA_PROVIDER_ID.to_string(),
    )
    .unwrap();
    catalog
        .merge(configured_qq_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID).unwrap())
        .unwrap();
    (catalog, config)
}
