//! Configuration scopes and typed context IDs.

use serde::{Deserialize, Serialize};

use crate::budgets::ConfigBudgets;
use crate::error::ConfigError;

fn validate_id(name: &str, value: &str, budgets: &ConfigBudgets) -> Result<(), ConfigError> {
    if value.is_empty() {
        return Err(ConfigError::ScopeUnsupported {
            reason: format!("{name} must be non-empty"),
        });
    }
    if value.len() > budgets.max_id_bytes {
        return Err(ConfigError::ScopeUnsupported {
            reason: format!("{name} exceeds max_id_bytes={}", budgets.max_id_bytes),
        });
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(ConfigError::ScopeUnsupported {
            reason: format!("{name} contains control characters"),
        });
    }
    Ok(())
}

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

id_newtype!(HostId);
id_newtype!(BotId);
id_newtype!(AccountId);
id_newtype!(GuildId);
id_newtype!(ChannelId);
id_newtype!(PluginInstanceId);
id_newtype!(ConfigProviderId);

/// Supported configuration scopes. Snapshot/revision are isolated per context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigScope {
    Global,
    Host,
    Bot,
    Account,
    Guild,
    Channel,
    PluginInstance,
}

/// Explicit typed context — never inferred from URL or file path strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigContext {
    pub scope: ConfigScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<HostId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<BotId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<AccountId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<GuildId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<ChannelId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_instance_id: Option<PluginInstanceId>,
}

impl ConfigContext {
    pub fn global() -> Self {
        Self {
            scope: ConfigScope::Global,
            host_id: None,
            bot_id: None,
            account_id: None,
            guild_id: None,
            channel_id: None,
            plugin_instance_id: None,
        }
    }

    pub fn plugin_instance(plugin_instance_id: impl Into<String>) -> Self {
        Self {
            scope: ConfigScope::PluginInstance,
            host_id: None,
            bot_id: None,
            account_id: None,
            guild_id: None,
            channel_id: None,
            plugin_instance_id: Some(PluginInstanceId::new(plugin_instance_id)),
        }
    }

    pub fn storage_key(&self) -> String {
        let mut parts = vec![format!("{:?}", self.scope).to_ascii_lowercase()];
        if let Some(id) = &self.host_id {
            parts.push(format!("host={}", id.as_str()));
        }
        if let Some(id) = &self.bot_id {
            parts.push(format!("bot={}", id.as_str()));
        }
        if let Some(id) = &self.account_id {
            parts.push(format!("account={}", id.as_str()));
        }
        if let Some(id) = &self.guild_id {
            parts.push(format!("guild={}", id.as_str()));
        }
        if let Some(id) = &self.channel_id {
            parts.push(format!("channel={}", id.as_str()));
        }
        if let Some(id) = &self.plugin_instance_id {
            parts.push(format!("plugin={}", id.as_str()));
        }
        parts.join("|")
    }

    pub fn validate(&self, budgets: &ConfigBudgets) -> Result<(), ConfigError> {
        if let Some(id) = &self.host_id {
            validate_id("host_id", id.as_str(), budgets)?;
        }
        if let Some(id) = &self.bot_id {
            validate_id("bot_id", id.as_str(), budgets)?;
        }
        if let Some(id) = &self.account_id {
            validate_id("account_id", id.as_str(), budgets)?;
        }
        if let Some(id) = &self.guild_id {
            validate_id("guild_id", id.as_str(), budgets)?;
        }
        if let Some(id) = &self.channel_id {
            validate_id("channel_id", id.as_str(), budgets)?;
        }
        if let Some(id) = &self.plugin_instance_id {
            validate_id("plugin_instance_id", id.as_str(), budgets)?;
        }

        match self.scope {
            ConfigScope::Global => Ok(()),
            ConfigScope::Host if self.host_id.is_some() => Ok(()),
            ConfigScope::Bot if self.bot_id.is_some() => Ok(()),
            ConfigScope::Account if self.account_id.is_some() => Ok(()),
            ConfigScope::Guild if self.guild_id.is_some() => Ok(()),
            ConfigScope::Channel if self.channel_id.is_some() => Ok(()),
            ConfigScope::PluginInstance if self.plugin_instance_id.is_some() => Ok(()),
            other => Err(ConfigError::ScopeUnsupported {
                reason: format!("scope {other:?} missing required identity"),
            }),
        }
    }
}
