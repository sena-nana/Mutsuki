//! Platform adapter contract.
//!
//! An adapter is a loadable plugin that serves `message/send`, optionally `message/recall` and
//! `media/upload`, for the accounts of one platform, and submits inbound events to
//! `flow/ingress`. It declares itself with a [`BotAdapterDescriptor`] carried as the
//! `mutsuki.bot.adapter@1` manifest extension, so Hosts, the console and delivery discover
//! adapters from the LoadPlan instead of hard-coded plugin ids.

use mutsuki_runtime_contracts::{PluginExtensionDescriptor, PluginManifest};
use serde::{Deserialize, Serialize};

use crate::{BotAccountRef, BotPlatform};

pub const BOT_ADAPTER_EXTENSION_ID: &str = "mutsuki.bot.adapter";
pub const BOT_ADAPTER_EXTENSION_VERSION: u32 = 1;

/// Runners that serve the adapter's outbound protocols.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotAdapterOperations {
    pub send_runner_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recall_runner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_runner_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotAdapterDescriptor {
    pub platform: BotPlatform,
    pub display_name: String,
    /// Accounts this adapter generation serves; empty while unconfigured.
    #[serde(default)]
    pub accounts: Vec<String>,
    pub operations: BotAdapterOperations,
    /// Config provider that owns the adapter's settings, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_provider_id: Option<String>,
}

impl BotAdapterDescriptor {
    pub fn into_plugin_extension(self) -> Result<PluginExtensionDescriptor, serde_json::Error> {
        Ok(PluginExtensionDescriptor {
            extension_id: BOT_ADAPTER_EXTENSION_ID.into(),
            version: BOT_ADAPTER_EXTENSION_VERSION,
            projection: mutsuki_runtime_contracts::ExtensionProjection::Universal,
            payload: serde_json::to_value(self)?,
        })
    }

    pub fn from_plugin_extension(
        extension: &PluginExtensionDescriptor,
    ) -> Result<Option<Self>, serde_json::Error> {
        if extension.extension_id != BOT_ADAPTER_EXTENSION_ID
            || extension.version != BOT_ADAPTER_EXTENSION_VERSION
        {
            return Ok(None);
        }
        serde_json::from_value(extension.payload.clone()).map(Some)
    }

    pub fn serves(&self, account: &BotAccountRef) -> bool {
        self.platform == account.platform
            && self
                .accounts
                .iter()
                .any(|account_id| account_id == &account.account_id)
    }
}

/// A discovered adapter together with the plugin that declared it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotAdapterEntry {
    pub plugin_id: String,
    pub descriptor: BotAdapterDescriptor,
}

/// Adapters declared by one LoadPlan, in plugin-id order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BotAdapterDirectory {
    entries: Vec<BotAdapterEntry>,
}

impl BotAdapterDirectory {
    /// Collects every `mutsuki.bot.adapter@1` extension; a malformed payload fails the plan.
    pub fn from_manifests<'a>(
        manifests: impl IntoIterator<Item = &'a PluginManifest>,
    ) -> Result<Self, String> {
        let mut entries = Vec::new();
        for manifest in manifests {
            for extension in &manifest.provides.extensions {
                let descriptor =
                    BotAdapterDescriptor::from_plugin_extension(extension).map_err(|error| {
                        format!(
                            "plugin {} declares an invalid {BOT_ADAPTER_EXTENSION_ID} \
                             extension: {error}",
                            manifest.plugin_id
                        )
                    })?;
                if let Some(descriptor) = descriptor {
                    entries.push(BotAdapterEntry {
                        plugin_id: manifest.plugin_id.to_string(),
                        descriptor,
                    });
                }
            }
        }
        entries.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[BotAdapterEntry] {
        &self.entries
    }

    /// The adapter serving `account`; an unknown or ambiguous account is a structured error.
    pub fn resolve(&self, account: &BotAccountRef) -> Result<&BotAdapterEntry, String> {
        let mut matches = self
            .entries
            .iter()
            .filter(|entry| entry.descriptor.serves(account));
        let first = matches.next().ok_or_else(|| {
            format!(
                "bot.adapter.account_unknown: no adapter serves {}:{}",
                account.platform.as_str(),
                account.account_id
            )
        })?;
        if let Some(second) = matches.next() {
            return Err(format!(
                "bot.adapter.account_ambiguous: {}:{} is served by {} and {}",
                account.platform.as_str(),
                account.account_id,
                first.plugin_id,
                second.plugin_id
            ));
        }
        Ok(first)
    }
}
