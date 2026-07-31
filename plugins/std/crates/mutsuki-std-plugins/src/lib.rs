use mutsuki_plugin_image_render_skia::{SkiaRenderConfig, SkiaRenderRunner};
use mutsuki_plugin_io_browser_chromium::{BrowserSnapshotRunner, ChromiumConfig};
use mutsuki_service_runtime::{
    ConfiguredPluginCatalog, ConfiguredPluginFactory, ServiceRuntimeBuilder, ServiceRuntimeResult,
};
use serde_json::Value;

pub struct MemoryResourcePluginFactory;

impl ConfiguredPluginFactory for MemoryResourcePluginFactory {
    fn plugin_id(&self) -> &str {
        mutsuki_plugin_resource_memory::PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        if !config.is_null() && config.as_object().is_none_or(|object| !object.is_empty()) {
            return Err("memory resource provider does not accept product configuration".into());
        }
        let manifest = mutsuki_plugin_resource_memory::loaded_plugin().manifest;
        Ok(
            builder.register_builtin_loaded_plugin_factory(manifest, || {
                Ok::<_, String>(mutsuki_plugin_resource_memory::loaded_plugin())
            }),
        )
    }
}

pub struct ChromiumPluginFactory;

impl ConfiguredPluginFactory for ChromiumPluginFactory {
    fn plugin_id(&self) -> &str {
        mutsuki_plugin_io_browser_chromium::PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: ChromiumConfig = serde_json::from_value(config.clone())
            .map_err(|error| format!("invalid Chromium plugin config: {error}"))?;
        config.validate()?;
        let manifest = mutsuki_plugin_io_browser_chromium::manifest();
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_fallible_runtime_services_runner(move |_client, resources| {
                BrowserSnapshotRunner::launch(config.clone(), resources)
                    .map(|runner| Box::new(runner) as Box<dyn mutsuki_runtime_core::Runner>)
            }))
    }
}

pub struct SkiaRenderPluginFactory;

impl ConfiguredPluginFactory for SkiaRenderPluginFactory {
    fn plugin_id(&self) -> &str {
        mutsuki_plugin_image_render_skia::PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: SkiaRenderConfig = serde_json::from_value(config.clone())
            .map_err(|error| format!("invalid Skia renderer config: {error}"))?;
        config.validate()?;
        let mut manifest = mutsuki_plugin_image_render_skia::manifest();
        manifest
            .requires
            .push(format!("resource_strategy:{}", config.output_provider_id));
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_fallible_runtime_services_runner(move |_client, resources| {
                SkiaRenderRunner::launch(config.clone(), resources)
                    .map(|runner| Box::new(runner) as Box<dyn mutsuki_runtime_core::Runner>)
            }))
    }
}

pub fn configured_std_plugin_catalog() -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog.register(MemoryResourcePluginFactory)?;
    catalog.register(ChromiumPluginFactory)?;
    catalog.register(SkiaRenderPluginFactory)?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_catalog_contains_memory_chromium_and_skia_factories() {
        configured_std_plugin_catalog().unwrap();
    }
}
