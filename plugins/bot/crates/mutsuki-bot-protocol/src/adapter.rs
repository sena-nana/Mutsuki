//! Platform adapter contract.
//!
//! An adapter is a loadable plugin that serves outbound endpoints (send, optionally recall and
//! upload) for the accounts of one platform, and submits inbound events to `flow/ingress`. It
//! declares itself with a [`BotAdapterDescriptor`] carried as the `mutsuki.bot.adapter@1`
//! manifest extension, so Hosts, the console and delivery discover adapters from the `LoadPlan`
//! instead of hard-coded plugin ids.
//!
//! Callers never address an adapter directly. They submit the account-routed protocols
//! (`message/send@2`, `message/recall@2`, `media/upload@2`) with a [`BotAccountRequest`]; the
//! single runner serving those resolves [`BotAdapterDirectory::route`] and calls the adapter's
//! endpoint by `runner_hint`. Endpoints are adapter-owned protocol ids because the Host admits one
//! runner per protocol; two adapters can therefore never share an endpoint protocol.

use mutsuki_runtime_contracts::{PluginExtensionDescriptor, PluginManifest};
use serde::{Deserialize, Serialize};

use crate::{
    BOT_MEDIA_UPLOAD_V2_PROTOCOL_ID, BOT_MESSAGE_RECALL_V2_PROTOCOL_ID,
    BOT_MESSAGE_SEND_V2_PROTOCOL_ID, BotAccountRef, BotPlatform,
};

pub const BOT_ADAPTER_EXTENSION_ID: &str = "mutsuki.bot.adapter";
pub const BOT_ADAPTER_EXTENSION_VERSION: u32 = 1;

/// Payload of the account-routed outbound protocols and of every adapter endpoint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotAccountRequest<T> {
    pub account: BotAccountRef,
    pub request: T,
}

/// Outbound operations an adapter may serve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotOutboundOperation {
    Send,
    Recall,
    Upload,
}

impl BotOutboundOperation {
    pub const ALL: [Self; 3] = [Self::Send, Self::Recall, Self::Upload];

    /// The account-routed protocol a caller submits for this operation.
    pub fn routed_protocol_id(self) -> &'static str {
        match self {
            Self::Send => BOT_MESSAGE_SEND_V2_PROTOCOL_ID,
            Self::Recall => BOT_MESSAGE_RECALL_V2_PROTOCOL_ID,
            Self::Upload => BOT_MEDIA_UPLOAD_V2_PROTOCOL_ID,
        }
    }

    pub fn from_routed_protocol_id(protocol_id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.routed_protocol_id() == protocol_id)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Recall => "recall",
            Self::Upload => "upload",
        }
    }
}

/// One adapter-owned endpoint: the protocol it accepts and the runner that serves it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotAdapterEndpoint {
    pub protocol_id: String,
    pub runner_id: String,
}

impl BotAdapterEndpoint {
    pub fn new(protocol_id: impl Into<String>, runner_id: impl Into<String>) -> Self {
        Self {
            protocol_id: protocol_id.into(),
            runner_id: runner_id.into(),
        }
    }
}

/// Endpoints that serve the adapter's outbound operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotAdapterOperations {
    pub send: BotAdapterEndpoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recall: Option<BotAdapterEndpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload: Option<BotAdapterEndpoint>,
}

impl BotAdapterOperations {
    pub fn endpoint(&self, operation: BotOutboundOperation) -> Option<&BotAdapterEndpoint> {
        match operation {
            BotOutboundOperation::Send => Some(&self.send),
            BotOutboundOperation::Recall => self.recall.as_ref(),
            BotOutboundOperation::Upload => self.upload.as_ref(),
        }
    }

    pub fn endpoints(&self) -> impl Iterator<Item = &BotAdapterEndpoint> {
        std::iter::once(&self.send)
            .chain(self.recall.as_ref())
            .chain(self.upload.as_ref())
    }
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

/// Where an account-routed operation goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotAdapterRoute {
    pub plugin_id: String,
    pub endpoint: BotAdapterEndpoint,
}

/// Adapters declared by one `LoadPlan`, in plugin-id order.
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

    /// Every endpoint protocol the adapters of this plan serve, deduplicated.
    pub fn endpoint_protocol_ids(&self) -> Vec<String> {
        let mut protocols = self
            .entries
            .iter()
            .flat_map(|entry| entry.descriptor.operations.endpoints())
            .map(|endpoint| endpoint.protocol_id.clone())
            .collect::<Vec<_>>();
        protocols.sort();
        protocols.dedup();
        protocols
    }

    /// Resolves the endpoint for `operation` on `account`; an adapter that does not serve the
    /// operation is a structured `bot.adapter.operation_unsupported` error.
    pub fn route(
        &self,
        operation: BotOutboundOperation,
        account: &BotAccountRef,
    ) -> Result<BotAdapterRoute, String> {
        let entry = self.resolve(account)?;
        let endpoint = entry
            .descriptor
            .operations
            .endpoint(operation)
            .ok_or_else(|| {
                format!(
                    "bot.adapter.operation_unsupported: {} does not serve {} for {}:{}",
                    entry.plugin_id,
                    operation.as_str(),
                    account.platform.as_str(),
                    account.account_id
                )
            })?;
        Ok(BotAdapterRoute {
            plugin_id: entry.plugin_id.clone(),
            endpoint: endpoint.clone(),
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(platform: BotPlatform, account: &str, protocol: &str) -> BotAdapterEntry {
        let descriptor = BotAdapterDescriptor {
            platform,
            display_name: account.into(),
            accounts: vec![account.into()],
            operations: BotAdapterOperations {
                send: BotAdapterEndpoint::new(protocol, format!("{protocol}.runner")),
                recall: None,
                upload: None,
            },
            config_provider_id: None,
        };
        // The manifest extension is the wire form; it must round-trip unchanged.
        let extension = descriptor.clone().into_plugin_extension().unwrap();
        assert_eq!(
            BotAdapterDescriptor::from_plugin_extension(&extension).unwrap(),
            Some(descriptor.clone())
        );
        BotAdapterEntry {
            plugin_id: format!("plugin.{account}"),
            descriptor,
        }
    }

    fn account(platform: BotPlatform, account_id: &str) -> BotAccountRef {
        BotAccountRef {
            account_id: account_id.into(),
            platform,
        }
    }

    #[test]
    fn route_selects_the_endpoint_of_the_adapter_serving_the_account() {
        let directory = BotAdapterDirectory {
            entries: vec![
                entry(BotPlatform::QqBot, "qq-main", "qq.send@1"),
                entry(BotPlatform::Sandbox, "sandbox", "sandbox.send@1"),
            ],
        };

        let route = directory
            .route(
                BotOutboundOperation::Send,
                &account(BotPlatform::Sandbox, "sandbox"),
            )
            .unwrap();
        assert_eq!(route.plugin_id, "plugin.sandbox");
        assert_eq!(route.endpoint.protocol_id, "sandbox.send@1");
        assert_eq!(route.endpoint.runner_id, "sandbox.send@1.runner");
        assert_eq!(
            directory.endpoint_protocol_ids(),
            vec!["qq.send@1".to_owned(), "sandbox.send@1".to_owned()]
        );

        // The account id alone does not route: the platform is part of the identity.
        let error = directory
            .route(
                BotOutboundOperation::Send,
                &account(BotPlatform::QqBot, "sandbox"),
            )
            .unwrap_err();
        assert!(error.starts_with("bot.adapter.account_unknown"), "{error}");
        let error = directory
            .route(
                BotOutboundOperation::Recall,
                &account(BotPlatform::QqBot, "qq-main"),
            )
            .unwrap_err();
        assert!(
            error.starts_with("bot.adapter.operation_unsupported"),
            "{error}"
        );
    }

    #[test]
    fn routed_protocol_ids_round_trip() {
        for operation in BotOutboundOperation::ALL {
            assert_eq!(
                BotOutboundOperation::from_routed_protocol_id(operation.routed_protocol_id()),
                Some(operation)
            );
        }
        assert_eq!(
            BotOutboundOperation::from_routed_protocol_id(crate::BOT_MESSAGE_SEND_PROTOCOL_ID),
            None
        );
    }
}
