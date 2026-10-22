//! `mutsuki.bot.platforms` `WebExtension`: one console surface for every platform adapter.
//!
//! Providers are resolved on every call, so an adapter enabled or reloaded after the console
//! started appears (or disappears) without restarting the console.

use std::path::PathBuf;
use std::sync::Arc;

use mutsuki_bot_console_protocol::platforms::{
    AccountActionResult, AccountsAction, AccountsList, AdaptersList, Changed, PLATFORMS_NAMESPACE,
    PLATFORMS_READ_CAPABILITY, PLATFORMS_WRITE_CAPABILITY,
};
use mutsuki_bot_management::{PlatformConsoleError, PlatformConsoleProvider};
use mutsuki_web_extension_api::{
    BundledAssets, BundledManifest, EventRegistry, ExtensionError, RpcRegistry, WebExtension,
    WebExtensionDescriptor,
};
use mutsuki_web_protocol::{
    AssetEntry, ExtensionManifest, RpcMethodDescriptor, TopicDescriptor, TypedTopic,
    WebFrontendAssets, ts_contract_module,
};

pub const PLUGIN_ID: &str = "mutsuki.bot.platforms";
pub const PLUGIN_VERSION: &str = "0.1.0";

/// Returns the providers of the adapters that are live right now.
pub type PlatformProviderSource =
    Arc<dyn Fn() -> Vec<Arc<dyn PlatformConsoleProvider>> + Send + Sync>;

pub struct PlatformsWebExtension {
    providers: PlatformProviderSource,
    assets_root: BundledAssets,
}

impl PlatformsWebExtension {
    pub fn new(providers: PlatformProviderSource) -> Self {
        Self {
            providers,
            assets_root: BundledAssets::default(),
        }
    }

    #[must_use]
    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root.set(root);
        self
    }
}

impl WebExtension for PlatformsWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        match self.frontend_assets() {
            Some(assets) => manifest(assets.manifest.assets),
            // Backend-only until the page bundle is staged: no entry for the shell to load.
            None => ExtensionManifest {
                entry: String::new(),
                ..manifest(Vec::new())
            },
        }
    }

    fn namespace(&self) -> String {
        PLATFORMS_NAMESPACE.into()
    }

    fn frontend_assets(&self) -> Option<WebFrontendAssets> {
        self.assets_root.resolve(manifest)
    }

    fn register_rpc(&self, registry: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let providers = self.providers.clone();
        registry.register_typed::<AdaptersList, _, _>(move |_context, _params| {
            let adapters = providers()
                .iter()
                .map(|provider| provider.adapter())
                .collect();
            async move { Ok(adapters) }
        });

        let providers = self.providers.clone();
        registry.register_typed::<AccountsList, _, _>(move |_context, params| {
            let providers = providers();
            async move {
                let mut accounts = Vec::new();
                for provider in providers {
                    if params
                        .adapter_id
                        .as_deref()
                        .is_some_and(|wanted| wanted != provider.adapter().adapter_id)
                    {
                        continue;
                    }
                    accounts.extend(provider.accounts().await.map_err(domain_error)?);
                }
                Ok(accounts)
            }
        });

        let providers = self.providers.clone();
        registry.register_typed::<AccountsAction, _, _>(move |context, params| {
            let provider = providers()
                .into_iter()
                .find(|provider| provider.adapter().adapter_id == params.adapter_id);
            async move {
                let provider = provider.ok_or_else(|| ExtensionError::Rpc {
                    code: "platform.adapter_unavailable".into(),
                    message: format!("adapter `{}` is not running", params.adapter_id),
                })?;
                let action = provider
                    .adapter()
                    .account_actions
                    .into_iter()
                    .find(|action| action.id == params.action)
                    .ok_or_else(|| ExtensionError::Rpc {
                        code: "platform.action_unknown".into(),
                        message: format!("unknown account action `{}`", params.action),
                    })?;
                context.require(&action.capability)?;
                provider
                    .action(context.principal_id(), params)
                    .await
                    .map_err(domain_error)
                    .map(|result: AccountActionResult| result)
            }
        });
        Ok(())
    }

    fn register_events(&self, registry: &mut EventRegistry) -> Result<(), ExtensionError> {
        registry.register_topic(Changed::TOPIC);
        Ok(())
    }
}

fn manifest(assets: Vec<AssetEntry>) -> ExtensionManifest {
    BundledManifest {
        id: PLUGIN_ID,
        version: PLUGIN_VERSION,
        entry: "index.js",
        capabilities: vec![
            PLATFORMS_READ_CAPABILITY.into(),
            PLATFORMS_WRITE_CAPABILITY.into(),
        ],
    }
    .build(assets)
}

fn domain_error(PlatformConsoleError { code, message }: PlatformConsoleError) -> ExtensionError {
    ExtensionError::Rpc { code, message }
}

/// The generated `web/src/generated/platforms.contract.gen.ts` module.
#[must_use]
pub fn ts_contract() -> String {
    ts_contract_module(
        "platformsApi",
        PLATFORMS_NAMESPACE,
        "../types",
        &[
            RpcMethodDescriptor::of::<AdaptersList>(),
            RpcMethodDescriptor::of::<AccountsList>(),
            RpcMethodDescriptor::of::<AccountsAction>(),
        ],
        &[TopicDescriptor::of::<Changed>()],
    )
}
