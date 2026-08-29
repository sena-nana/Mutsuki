//! Default Web configuration plugin.
//!
//! Registers typed RPC:
//! - config / providers.list
//! - config / navigation.list
//! - config / schema.get
//! - config / snapshot.read
//! - config / validate
//! - config / apply
//! - config / metrics
//!
//! Frontend assets generate forms from ConfigDescriptor (Koishi-like shell + LiliaUI tokens).
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls,
    clippy::ref_option,
    clippy::return_self_not_must_use,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_config_service::{
    ConfigApplyRequest, ConfigContext, ConfigProviderId, ConfigService, ConfigValue,
};
use mutsuki_web_extension::{
    ExtensionError, RpcRegistry, WebExtension, WebExtensionDescriptor, content_hash,
};
use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, JsonValue, WEB_PROTOCOL_VERSION,
    WebFrontendAssets,
};

pub const PLUGIN_ID: &str = "config";
pub const PLUGIN_VERSION: &str = "0.1.0";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ConfigNavigationItem {
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ConfigNavigationGroup {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub items: Vec<ConfigNavigationItem>,
}

/// Backend WebExtension that fronts a shared ConfigService.
pub struct ConfigWebExtension {
    service: Arc<ConfigService>,
    assets_root: Option<PathBuf>,
    capabilities: Vec<String>,
    visible_providers: Option<BTreeSet<String>>,
    navigation_groups: Vec<ConfigNavigationGroup>,
}

impl ConfigWebExtension {
    pub fn new(service: Arc<ConfigService>) -> Self {
        Self {
            service,
            assets_root: None,
            capabilities: vec![
                mutsuki_config_service::capability::SCHEMA_READ.into(),
                mutsuki_config_service::capability::VALUE_READ.into(),
                mutsuki_config_service::capability::VALUE_WRITE.into(),
                mutsuki_config_service::capability::SECRET_WRITE.into(),
                mutsuki_config_service::capability::APPLY.into(),
                mutsuki_config_service::capability::RELOAD.into(),
            ],
            visible_providers: None,
            navigation_groups: Vec::new(),
        }
    }

    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root = Some(root.into());
        self
    }

    /// Restricts both discovery and direct RPC access to product-selected providers.
    pub fn with_visible_providers(
        mut self,
        providers: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.visible_providers = Some(providers.into_iter().map(Into::into).collect());
        self
    }

    /// Defines presentation-only provider groups without changing ConfigService ownership.
    #[must_use]
    pub fn with_navigation_groups(
        mut self,
        groups: impl IntoIterator<Item = ConfigNavigationGroup>,
    ) -> Self {
        self.navigation_groups = groups.into_iter().collect();
        self
    }

    fn block_on<F, T>(fut: F) -> Result<T, ExtensionError>
    where
        F: std::future::Future<Output = Result<T, mutsuki_config_service::ConfigError>>,
    {
        // Sync RPC boundary: providers used here are in-memory / off Bot hot path.
        // Avoid nested tokio runtimes (axum may run current-thread).
        futures_executor::block_on(fut).map_err(map_config_error)
    }
}

impl WebExtension for ConfigWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        ExtensionManifest {
            manifest_version: EXTENSION_MANIFEST_VERSION,
            id: PLUGIN_ID.into(),
            version: PLUGIN_VERSION.into(),
            entry: "index.js".into(),
            capabilities: self.capabilities.clone(),
            permissions: vec!["pages".into(), "navigation".into()],
            assets: self
                .frontend_assets()
                .map(|assets| assets.manifest.assets)
                .unwrap_or_default(),
            protocol_version: WEB_PROTOCOL_VERSION.into(),
        }
    }

    fn frontend_assets(&self) -> Option<WebFrontendAssets> {
        let root = self.assets_root.as_ref()?;
        let manifest = load_or_synthesize_manifest(root).ok()?;
        Some(WebFrontendAssets {
            manifest,
            root_dir: root.clone(),
        })
    }

    fn register_rpc(&self, ctx: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let service = self.service.clone();
        let visible_providers = self.visible_providers.clone();
        let navigation_groups = self.navigation_groups.clone();
        ctx.register_contextual("providers.list", {
            let service = service.clone();
            let visible_providers = visible_providers.clone();
            move |context, _params| {
                context.require(mutsuki_config_service::capability::SCHEMA_READ)?;
                let mut list = service
                    .list_providers(context.capabilities())
                    .map_err(map_config_error)?;
                if let Some(visible) = &visible_providers {
                    list.retain(|provider| visible.contains(provider.as_str()));
                }
                Ok(serde_json::to_value(list).unwrap_or_default())
            }
        });

        ctx.register_contextual("navigation.list", {
            let service = service.clone();
            let visible_providers = visible_providers.clone();
            move |context, _params| {
                context.require(mutsuki_config_service::capability::SCHEMA_READ)?;
                let mut providers = service
                    .list_providers(context.capabilities())
                    .map_err(map_config_error)?;
                if let Some(visible) = &visible_providers {
                    providers.retain(|provider| visible.contains(provider.as_str()));
                }
                let groups = visible_navigation_groups(&providers, &navigation_groups);
                Ok(serde_json::to_value(groups).unwrap_or_default())
            }
        });

        ctx.register_contextual("schema.get", {
            let service = service.clone();
            let visible_providers = visible_providers.clone();
            move |context, params| {
                context.require(mutsuki_config_service::capability::SCHEMA_READ)?;
                let provider_id = required_str(&params, "provider_id")?;
                require_visible_provider(&visible_providers, &provider_id)?;
                let schema = service
                    .get_schema(&provider_id, context.capabilities())
                    .map_err(map_config_error)?;
                Ok(serde_json::to_value(schema).unwrap_or_default())
            }
        });

        ctx.register_contextual("snapshot.read", {
            let service = service.clone();
            let visible_providers = visible_providers.clone();
            move |call_context, params| {
                call_context.require(mutsuki_config_service::capability::VALUE_READ)?;
                let provider_id = required_str(&params, "provider_id")?;
                require_visible_provider(&visible_providers, &provider_id)?;
                let context = context_from_params(&params)?;
                let snapshot = ConfigWebExtension::block_on(service.read(
                    &provider_id,
                    context,
                    call_context.capabilities(),
                ))?;
                Ok(serde_json::to_value(snapshot).unwrap_or_default())
            }
        });

        ctx.register_contextual("validate", {
            let service = service.clone();
            let visible_providers = visible_providers.clone();
            move |call_context, params| {
                call_context.require(mutsuki_config_service::capability::VALUE_WRITE)?;
                let provider_id = required_str(&params, "provider_id")?;
                require_visible_provider(&visible_providers, &provider_id)?;
                let context = context_from_params(&params)?;
                let candidate = candidate_from_params(&params)?;
                let result = ConfigWebExtension::block_on(service.validate(
                    &provider_id,
                    candidate,
                    context,
                    call_context.capabilities(),
                ))?;
                Ok(serde_json::to_value(result).unwrap_or_default())
            }
        });

        ctx.register_contextual("apply", {
            let service = service.clone();
            let visible_providers = visible_providers.clone();
            move |call_context, params| {
                call_context.require(mutsuki_config_service::capability::APPLY)?;
                let provider_id = required_str(&params, "provider_id")?;
                require_visible_provider(&visible_providers, &provider_id)?;
                let context = context_from_params(&params)?;
                let request = apply_request_from_params(&params)?;
                let result = ConfigWebExtension::block_on(service.apply(
                    &provider_id,
                    request,
                    context,
                    call_context.capabilities(),
                ))?;
                Ok(serde_json::to_value(result).unwrap_or_default())
            }
        });

        ctx.register_contextual("metrics", {
            let service = service.clone();
            move |context, _params| {
                context.require(mutsuki_config_service::capability::SCHEMA_READ)?;
                Ok(serde_json::to_value(service.metrics_snapshot()).unwrap_or_default())
            }
        });

        Ok(())
    }

    fn register_events(
        &self,
        ctx: &mut mutsuki_web_extension::EventRegistry,
    ) -> Result<(), ExtensionError> {
        ctx.register_topic("revision_changed");
        Ok(())
    }
}

fn visible_navigation_groups(
    providers: &[ConfigProviderId],
    configured: &[ConfigNavigationGroup],
) -> Vec<ConfigNavigationGroup> {
    if configured.is_empty() {
        if providers.is_empty() {
            return Vec::new();
        }
        return vec![ConfigNavigationGroup {
            label: None,
            items: providers
                .iter()
                .map(|provider| ConfigNavigationItem {
                    provider_id: provider.as_str().to_owned(),
                    label: None,
                })
                .collect(),
        }];
    }

    // Items without a registered config provider are kept: they may name plugins
    // that expose no schema (display-only hub entries). The frontend only renders
    // those when the plugin id shows up in control.plugin_list.
    let mut included = BTreeSet::new();
    let mut groups = configured
        .iter()
        .filter_map(|group| {
            let items = group
                .items
                .iter()
                .filter(|item| included.insert(item.provider_id.clone()))
                .cloned()
                .collect::<Vec<_>>();
            (!items.is_empty()).then(|| ConfigNavigationGroup {
                label: group.label.clone(),
                items,
            })
        })
        .collect::<Vec<_>>();
    let remaining = providers
        .iter()
        .filter(|provider| !included.contains(provider.as_str()))
        .map(|provider| ConfigNavigationItem {
            provider_id: provider.as_str().to_owned(),
            label: None,
        })
        .collect::<Vec<_>>();
    if !remaining.is_empty() {
        groups.push(ConfigNavigationGroup {
            label: None,
            items: remaining,
        });
    }
    groups
}

fn require_visible_provider(
    visible: &Option<BTreeSet<String>>,
    provider_id: &str,
) -> Result<(), ExtensionError> {
    if visible
        .as_ref()
        .is_some_and(|providers| !providers.contains(provider_id))
    {
        return Err(ExtensionError::Registration(
            "configuration provider is not exposed by this application".into(),
        ));
    }
    Ok(())
}

fn map_config_error(err: mutsuki_config_service::ConfigError) -> ExtensionError {
    ExtensionError::Registration(serde_json::to_string(&err).unwrap_or_else(|_| err.to_string()))
}

fn required_str(params: &JsonValue, key: &str) -> Result<String, ExtensionError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ExtensionError::Registration(format!("missing {key}")))
}

fn context_from_params(params: &JsonValue) -> Result<ConfigContext, ExtensionError> {
    params.get("context").cloned().map_or_else(
        || Ok(ConfigContext::global()),
        |value| {
            serde_json::from_value(value)
                .map_err(|error| ExtensionError::Registration(error.to_string()))
        },
    )
}

fn candidate_from_params(params: &JsonValue) -> Result<ConfigValue, ExtensionError> {
    let raw = params
        .get("candidate")
        .cloned()
        .ok_or_else(|| ExtensionError::Registration("missing candidate".into()))?;
    config_value_from_json(raw)
}

fn apply_request_from_params(params: &JsonValue) -> Result<ConfigApplyRequest, ExtensionError> {
    let request_value = params
        .get("request")
        .cloned()
        .ok_or_else(|| ExtensionError::Registration("missing request".into()))?;
    let candidate = request_value
        .get("candidate")
        .cloned()
        .ok_or_else(|| ExtensionError::Registration("missing candidate".into()))?;
    let expected = request_value
        .get("expected_revision")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let dry_run = request_value
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(ConfigApplyRequest {
        candidate: config_value_from_json(candidate)?,
        expected_revision: mutsuki_config_service::ConfigRevision(expected),
        dry_run,
    })
}

fn config_value_from_json(raw: JsonValue) -> Result<ConfigValue, ExtensionError> {
    if raw.get("type").is_some() {
        serde_json::from_value(raw).map_err(|e| ExtensionError::Registration(e.to_string()))
    } else {
        Ok(ConfigValue::from_json(&raw))
    }
}

fn load_or_synthesize_manifest(root: &Path) -> Result<ExtensionManifest, ExtensionError> {
    let manifest_path = root.join("manifest.json");
    if manifest_path.exists() {
        let bytes =
            std::fs::read(&manifest_path).map_err(|e| ExtensionError::Manifest(e.to_string()))?;
        return serde_json::from_slice(&bytes).map_err(|e| ExtensionError::Manifest(e.to_string()));
    }
    let entry = root.join("index.js");
    let bytes = std::fs::read(&entry).map_err(|e| ExtensionError::Manifest(e.to_string()))?;
    Ok(ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![
            mutsuki_config_service::capability::SCHEMA_READ.into(),
            mutsuki_config_service::capability::VALUE_READ.into(),
            mutsuki_config_service::capability::VALUE_WRITE.into(),
            mutsuki_config_service::capability::SECRET_WRITE.into(),
            mutsuki_config_service::capability::APPLY.into(),
        ],
        permissions: vec!["pages".into(), "navigation".into()],
        assets: vec![AssetEntry {
            path: "index.js".into(),
            content_hash: content_hash(&bytes),
            bytes: bytes.len() as u64,
        }],
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    })
}

/// Write bundled frontend assets for the default config web plugin.
pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(out_dir)?;
    let js = include_str!("../assets/index.js");
    let css = include_str!("../assets/mutsuki-ui.css");
    std::fs::write(out_dir.join("index.js"), js)?;
    std::fs::write(out_dir.join("mutsuki-ui.css"), css)?;
    let entry_bytes = js.as_bytes();
    let css_bytes = css.as_bytes();
    let manifest = ExtensionManifest {
        manifest_version: EXTENSION_MANIFEST_VERSION,
        id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        entry: "index.js".into(),
        capabilities: vec![
            mutsuki_config_service::capability::SCHEMA_READ.into(),
            mutsuki_config_service::capability::VALUE_READ.into(),
            mutsuki_config_service::capability::VALUE_WRITE.into(),
            mutsuki_config_service::capability::SECRET_WRITE.into(),
            mutsuki_config_service::capability::APPLY.into(),
        ],
        permissions: vec!["pages".into(), "navigation".into()],
        assets: vec![
            AssetEntry {
                path: "index.js".into(),
                content_hash: content_hash(entry_bytes),
                bytes: entry_bytes.len() as u64,
            },
            AssetEntry {
                path: "mutsuki-ui.css".into(),
                content_hash: content_hash(css_bytes),
                bytes: css_bytes.len() as u64,
            },
        ],
        protocol_version: WEB_PROTOCOL_VERSION.into(),
    };
    std::fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest"),
    )?;
    Ok(out_dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_groups_keep_explicit_order_and_schema_less_items() {
        let providers = vec![
            ConfigProviderId::new("mutsuki.product"),
            ConfigProviderId::new("plugin.qq"),
        ];
        let groups = vec![
            ConfigNavigationGroup {
                label: None,
                items: vec![ConfigNavigationItem {
                    provider_id: "mutsuki.product".into(),
                    label: Some("Mutsuki".into()),
                }],
            },
            ConfigNavigationGroup {
                label: Some("插件".into()),
                items: ["plugin.missing", "plugin.qq"]
                    .map(|provider_id| ConfigNavigationItem {
                        provider_id: provider_id.into(),
                        label: None,
                    })
                    .into(),
            },
        ];

        let visible = visible_navigation_groups(&providers, &groups);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].items[0].label.as_deref(), Some("Mutsuki"));
        assert_eq!(visible[1].label.as_deref(), Some("插件"));
        // Items without a schema are kept so products can name schema-less
        // plugins; the frontend gates them on control.plugin_list.
        assert_eq!(visible[1].items[0].provider_id, "plugin.missing");
        assert_eq!(visible[1].items[1].provider_id, "plugin.qq");
    }

    #[test]
    fn navigation_groups_name_schema_less_plugins_without_providers() {
        let groups = vec![ConfigNavigationGroup {
            label: Some("扩展".into()),
            items: vec![ConfigNavigationItem {
                provider_id: "mutsuki.bot.router.flow".into(),
                label: Some("流程路由".into()),
            }],
        }];

        let visible = visible_navigation_groups(&[], &groups);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].label.as_deref(), Some("扩展"));
        assert_eq!(visible[0].items[0].label.as_deref(), Some("流程路由"));
    }
}
