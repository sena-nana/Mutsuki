use mutsuki_bot_qq_service_host_integration::configured_qq_plugin_catalog;
use mutsuki_bot_service_host_integration::{
    DEFAULT_MEDIA_PROVIDER_ID, configured_bot_plugin_catalog,
};
use mutsuki_service_runtime::ConfiguredPluginCatalog;

/// Core Bot owners plus the QQ adapter, as a QQ product assembles them.
pub fn bot_catalog_with_qq() -> ConfiguredPluginCatalog {
    let mut catalog = configured_bot_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID.to_string()).unwrap();
    catalog
        .merge(configured_qq_plugin_catalog(DEFAULT_MEDIA_PROVIDER_ID).unwrap())
        .unwrap();
    catalog
}
