//! The product's owner registry: one entry per owner plugin the product configures through its
//! own ConfigService document. Provider registration, owner selections, the `runtime_plugins`
//! guard, the HTTP egress binding, co-reload and console navigation are all derived from this
//! list, so adding a platform owner is one entry here plus its catalog merge in `lib.rs`.

use std::sync::Arc;

use mutsuki_agent_service_host_integration::{
    AGENT_CONNECTIONS_PLUGIN_ID, LOCAL_AGENT_API_KEY, LOCAL_AGENT_API_KEY_FIELD,
    LOCAL_AGENT_CONFIG_PROVIDER_ID, LocalAgentConfig, LocalAgentConfigProvider,
    local_agent_config_descriptor, local_agent_config_value,
};
use mutsuki_bot_service_host_integration::{
    BOT_COMMAND_PLUGIN_ID, BOT_INTERACTION_PLUGIN_ID, BOT_SANDBOX_PLUGIN_ID,
    link_card_config_descriptor, link_card_config_value,
};
use mutsuki_config_service::{
    ConfigApplyMode, ConfigContext, ConfigProvider, ConfigValue, MemoryConfigProvider,
};
use mutsuki_plugin_bot_adapter_qqbot::{
    QQ_CLIENT_SECRET_FIELD, QQ_CLIENT_SECRET_KEY, QQBOT_ADAPTER_PLUGIN_ID, QqBotConfig,
    qq_config_descriptor, qq_config_value,
};
use mutsuki_plugin_bot_agent::{
    BOT_AGENT_BRIDGE_PLUGIN_ID, BotAgentConfig, bot_agent_config_schema,
};
use mutsuki_plugin_bot_bilibili::{
    BILIBILI_COOKIE_FIELD, BILIBILI_COOKIE_KEY, BilibiliConfig, PLUGIN_ID as BILIBILI_PLUGIN_ID,
    bilibili_config_descriptor, bilibili_config_value,
};
use mutsuki_plugin_bot_bilibili_workshop::{PLUGIN_ID as WORKSHOP_PLUGIN_ID, WORKSHOP_DOMAINS};
use mutsuki_plugin_bot_event_router::BOT_FLOW_ROUTER_PLUGIN_ID;
use mutsuki_plugin_bot_mihuashi::{MIHUASHI_DOMAIN, PLUGIN_ID as MIHUASHI_PLUGIN_ID};
use mutsuki_plugin_io_http_client::{HttpSecretGrant, PLUGIN_ID as HTTP_CLIENT_PLUGIN_ID};
use mutsuki_service_config::HostSecretStore;

use crate::product_config::secret_backed_provider;

/// Console navigation section an owner's settings page belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConsoleSection {
    /// Platform accounts and content sources ("接入").
    Access,
    /// Model, Agent and reply settings ("助手").
    Assistant,
}

/// One owner plugin whose settings the product stores as a ConfigService document.
pub(crate) struct ProductOwner {
    /// Config provider id; it is also the configured plugin selection id.
    pub(crate) provider_id: &'static str,
    pub(crate) context: fn() -> ConfigContext,
    pub(crate) provider: fn(&HostSecretStore) -> Arc<dyn ConfigProvider>,
    /// Hosts the owner's platform plugins fetch through the product's std HTTP client.
    pub(crate) http_domains: &'static [&'static str],
    /// Secrets the HTTP client may attach (and capture) for this owner.
    pub(crate) http_secret_grants: fn() -> Vec<HttpSecretGrant>,
    /// Owners whose running selection must be reapplied together with this one.
    pub(crate) reload_with: &'static [&'static str],
    pub(crate) console: Option<(ConsoleSection, &'static str)>,
}

/// Workspace plugins the product enables with `workspace_enabled`; they have no owner document.
pub(crate) const WORKSPACE_PLUGIN_IDS: &[&str] = &[
    AGENT_CONNECTIONS_PLUGIN_ID,
    BOT_FLOW_ROUTER_PLUGIN_ID,
    BOT_SANDBOX_PLUGIN_ID,
    BOT_COMMAND_PLUGIN_ID,
    BOT_INTERACTION_PLUGIN_ID,
];

fn global() -> ConfigContext {
    ConfigContext::global()
}

fn no_grants() -> Vec<HttpSecretGrant> {
    Vec::new()
}

pub(crate) fn product_owners() -> Vec<ProductOwner> {
    vec![
        ProductOwner {
            provider_id: QQBOT_ADAPTER_PLUGIN_ID,
            context: global,
            provider: |secrets| {
                Arc::new(secret_backed_provider(
                    qq_config_descriptor(QQBOT_ADAPTER_PLUGIN_ID),
                    qq_config_value(false, &QqBotConfig::default()),
                    secrets,
                    &[(QQ_CLIENT_SECRET_FIELD, QQ_CLIENT_SECRET_KEY)],
                ))
            },
            http_domains: &[],
            http_secret_grants: no_grants,
            reload_with: &[],
            console: Some((ConsoleSection::Access, "QQ 登录")),
        },
        ProductOwner {
            provider_id: MIHUASHI_PLUGIN_ID,
            context: global,
            provider: |_| {
                Arc::new(MemoryConfigProvider::new(
                    link_card_config_descriptor(MIHUASHI_PLUGIN_ID, "米画师"),
                    link_card_config_value(false),
                    ConfigApplyMode::HotReload,
                ))
            },
            http_domains: &[MIHUASHI_DOMAIN],
            http_secret_grants: no_grants,
            reload_with: &[],
            console: Some((ConsoleSection::Access, "米画师")),
        },
        ProductOwner {
            provider_id: BILIBILI_PLUGIN_ID,
            context: global,
            provider: |secrets| {
                Arc::new(secret_backed_provider(
                    bilibili_config_descriptor(),
                    bilibili_config_value(false, &BilibiliConfig::default()),
                    secrets,
                    &[(BILIBILI_COOKIE_FIELD, BILIBILI_COOKIE_KEY)],
                ))
            },
            http_domains: mutsuki_plugin_bot_bilibili::HTTP_DOMAINS,
            http_secret_grants: || {
                vec![HttpSecretGrant {
                    secret_key: BILIBILI_COOKIE_KEY.into(),
                    domains: vec![mutsuki_plugin_bot_bilibili::COOKIE_DOMAIN.into()],
                    capture: true,
                }]
            },
            reload_with: &[],
            console: Some((ConsoleSection::Access, "B 站")),
        },
        ProductOwner {
            provider_id: WORKSHOP_PLUGIN_ID,
            context: global,
            provider: |_| {
                Arc::new(MemoryConfigProvider::new(
                    link_card_config_descriptor(WORKSHOP_PLUGIN_ID, "B 站工房"),
                    link_card_config_value(false),
                    ConfigApplyMode::HotReload,
                ))
            },
            http_domains: &WORKSHOP_DOMAINS,
            http_secret_grants: no_grants,
            reload_with: &[],
            console: Some((ConsoleSection::Access, "B 站工房")),
        },
        ProductOwner {
            provider_id: LOCAL_AGENT_CONFIG_PROVIDER_ID,
            context: global,
            provider: |secrets| {
                Arc::new(LocalAgentConfigProvider::new(secret_backed_provider(
                    local_agent_config_descriptor(),
                    local_agent_config_value(false, &LocalAgentConfig::default()),
                    secrets,
                    &[(LOCAL_AGENT_API_KEY_FIELD, LOCAL_AGENT_API_KEY)],
                )))
            },
            http_domains: &[],
            http_secret_grants: no_grants,
            // The Bot Agent bridge resolves its model through the local engine it binds to.
            reload_with: &[BOT_AGENT_BRIDGE_PLUGIN_ID],
            console: Some((ConsoleSection::Assistant, "模型")),
        },
        ProductOwner {
            provider_id: BOT_AGENT_BRIDGE_PLUGIN_ID,
            context: || ConfigContext::plugin_instance("default"),
            provider: |_| {
                Arc::new(MemoryConfigProvider::new(
                    bot_agent_config_schema(),
                    ConfigValue::from_json(
                        &serde_json::to_value(BotAgentConfig::default())
                            .expect("Bot Agent config serializes"),
                    ),
                    ConfigApplyMode::HotReload,
                ))
            },
            http_domains: &[],
            http_secret_grants: no_grants,
            reload_with: &[],
            console: Some((ConsoleSection::Assistant, "回复")),
        },
    ]
}

pub(crate) fn product_owner(provider_id: &str) -> Option<ProductOwner> {
    product_owners()
        .into_iter()
        .find(|owner| owner.provider_id == provider_id)
}

/// Plugins the product selects itself; `runtime_plugins` must never configure them.
pub(crate) fn is_product_owner_plugin(id: &str) -> bool {
    id == HTTP_CLIENT_PLUGIN_ID
        || WORKSPACE_PLUGIN_IDS.contains(&id)
        || product_owners().iter().any(|owner| owner.provider_id == id)
}

/// `(provider_id, label)` of every owner shown in `section`, in registry order.
pub(crate) fn console_items(section: ConsoleSection) -> Vec<(&'static str, &'static str)> {
    product_owners()
        .into_iter()
        .filter_map(|owner| match owner.console {
            Some((owner_section, label)) if owner_section == section => {
                Some((owner.provider_id, label))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn owner_registry_is_consistent() {
        let owners = product_owners();
        let ids = owners
            .iter()
            .map(|owner| owner.provider_id)
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), owners.len(), "owner ids are unique");
        for owner in &owners {
            assert!(!WORKSPACE_PLUGIN_IDS.contains(&owner.provider_id));
            assert!(is_product_owner_plugin(owner.provider_id));
            for co_reloaded in owner.reload_with {
                assert!(ids.contains(co_reloaded), "{co_reloaded} is an owner");
            }
            // A secret grant is only reachable on a host the owner also allows.
            for grant in (owner.http_secret_grants)() {
                for domain in &grant.domains {
                    assert!(owner.http_domains.contains(&domain.as_str()));
                }
            }
        }
    }

    /// Loadable Bot plugins are named `mutsuki.plugin.bot.<crate suffix>`; library, service and
    /// protocol ids keep their own `mutsuki.bot.*` namespaces.
    #[test]
    fn first_party_bot_plugin_ids_follow_the_plugin_namespace() {
        for (plugin_id, crate_suffix) in [
            (QQBOT_ADAPTER_PLUGIN_ID, "adapter-qqbot"),
            (BOT_FLOW_ROUTER_PLUGIN_ID, "flow"),
            (BOT_COMMAND_PLUGIN_ID, "command"),
            (BOT_INTERACTION_PLUGIN_ID, "interaction"),
            (BOT_AGENT_BRIDGE_PLUGIN_ID, "agent"),
            (BILIBILI_PLUGIN_ID, "bilibili"),
            (WORKSHOP_PLUGIN_ID, "bilibili-workshop"),
            (MIHUASHI_PLUGIN_ID, "mihuashi"),
            (
                mutsuki_bot_service_host_integration::BOT_SANDBOX_PLUGIN_ID,
                "adapter-sandbox",
            ),
            (
                mutsuki_plugin_bot_flow_agent_tool::PLUGIN_ID,
                "flow-agent-tool",
            ),
        ] {
            assert_eq!(plugin_id, format!("mutsuki.plugin.bot.{crate_suffix}"));
        }
    }

    #[test]
    fn every_owner_provider_registers_under_its_id() {
        let secrets = mutsuki_service_config::ServiceConfig::default().host_secret_store();
        for owner in product_owners() {
            assert_eq!(
                (owner.provider)(&secrets).descriptor().provider_id.as_str(),
                owner.provider_id
            );
        }
    }
}
