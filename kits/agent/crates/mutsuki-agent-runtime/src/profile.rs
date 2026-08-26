use std::collections::BTreeSet;

use mutsuki_agent_contracts::{
    AgentBudget, AgentContextPolicy, AgentError, AgentKnowledgePolicy, AgentPermissionPolicy,
    AgentPersistenceDistributionPolicy, AgentProfilePlugin, AgentPromptFragment,
    AgentProtocolAdapterSelection, AgentProviderInstance, AgentRuntimeMode, AgentRuntimePolicy,
    AgentRuntimeProfile, AgentSkillPolicy,
};

pub struct AgentRuntimeProfileBuilder {
    profile: AgentRuntimeProfile,
}

impl AgentRuntimeProfileBuilder {
    pub fn new(profile_id: impl Into<String>) -> Self {
        Self {
            profile: AgentRuntimeProfile {
                profile_id: profile_id.into(),
                mode: AgentRuntimeMode::Production,
                runtime: AgentRuntimePolicy::default(),
                adapters: Vec::new(),
                providers: Vec::new(),
                plugins: Vec::new(),
                system_instructions: Vec::new(),
                prompt_fragments: Vec::new(),
                context: AgentContextPolicy::default(),
                permissions: AgentPermissionPolicy::default(),
                budget: AgentBudget::default(),
                persistence_distribution: AgentPersistenceDistributionPolicy::default(),
                skill: Default::default(),
                knowledge: Default::default(),
            },
        }
    }

    pub fn mode(mut self, mode: AgentRuntimeMode) -> Self {
        self.profile.mode = mode;
        self
    }

    pub fn runtime_policy(mut self, policy: AgentRuntimePolicy) -> Self {
        self.profile.runtime = policy;
        self
    }

    pub fn provider(mut self, provider: AgentProviderInstance) -> Self {
        self.profile.providers.push(provider);
        self
    }

    pub fn adapter(mut self, adapter: AgentProtocolAdapterSelection) -> Self {
        self.profile.adapters.push(adapter);
        self
    }

    pub fn plugin(mut self, plugin: AgentProfilePlugin) -> Self {
        self.profile.plugins.push(plugin);
        self
    }

    pub fn system_instruction(mut self, instruction: impl Into<String>) -> Self {
        self.profile.system_instructions.push(instruction.into());
        self
    }

    pub fn prompt_fragment(mut self, fragment: AgentPromptFragment) -> Self {
        self.profile.prompt_fragments.push(fragment);
        self
    }

    pub fn context_policy(mut self, policy: AgentContextPolicy) -> Self {
        self.profile.context = policy;
        self
    }

    pub fn permission_policy(mut self, policy: AgentPermissionPolicy) -> Self {
        self.profile.permissions = policy;
        self
    }

    pub fn budget_policy(mut self, budget: AgentBudget) -> Self {
        self.profile.budget = budget;
        self
    }

    pub fn persistence_distribution(mut self, policy: AgentPersistenceDistributionPolicy) -> Self {
        self.profile.persistence_distribution = policy;
        self
    }

    pub fn skill_policy(mut self, policy: AgentSkillPolicy) -> Self {
        self.profile.skill = policy;
        self
    }

    pub fn knowledge_policy(mut self, policy: AgentKnowledgePolicy) -> Self {
        self.profile.knowledge = policy;
        self
    }

    pub fn build(self) -> Result<AgentRuntimeProfile, AgentError> {
        validate_agent_runtime_profile(&self.profile)?;
        Ok(self.profile)
    }
}

pub fn validate_agent_runtime_profile(profile: &AgentRuntimeProfile) -> Result<(), AgentError> {
    require_id(&profile.profile_id, "profile id")?;
    if profile.runtime.max_concurrent_turns == 0
        || profile.runtime.max_parallel_context_providers == 0
        || profile.runtime.max_parallel_tools == 0
        || profile.runtime.cancellation_grace_ms == 0
    {
        return Err(AgentError::invalid_input(
            "runtime concurrency and cancellation limits must be non-zero",
        ));
    }

    let provider_ids = unique_ids(
        profile
            .providers
            .iter()
            .map(|provider| provider.instance_id.as_str()),
        "provider instance",
    )?;
    for provider in &profile.providers {
        require_id(&provider.adapter_id, "provider adapter id")?;
        if profile.mode == AgentRuntimeMode::Production && provider.test_only {
            return Err(AgentError::new(
                "agent.profile.test_provider_in_production",
                format!(
                    "test-only provider `{}` cannot be registered in a production profile",
                    provider.instance_id
                ),
            ));
        }
    }

    let mut protocols = BTreeSet::new();
    for adapter in &profile.adapters {
        require_id(&adapter.protocol_family, "adapter protocol family")?;
        require_id(&adapter.adapter_id, "adapter id")?;
        require_id(
            &adapter.provider_instance_id,
            "adapter provider instance id",
        )?;
        require_id(&adapter.model, "adapter model")?;
        if !protocols.insert(adapter.protocol_family.as_str()) {
            return Err(AgentError::invalid_input(format!(
                "duplicate adapter protocol family `{}`",
                adapter.protocol_family
            )));
        }
        if !provider_ids.contains(adapter.provider_instance_id.as_str())
            || adapter
                .fallback_provider_instance_ids
                .iter()
                .any(|provider| !provider_ids.contains(provider.as_str()))
        {
            return Err(AgentError::invalid_input(format!(
                "adapter `{}` references an unknown provider instance",
                adapter.adapter_id
            )));
        }
        let provider = profile
            .providers
            .iter()
            .find(|provider| provider.instance_id == adapter.provider_instance_id)
            .expect("provider membership checked above");
        if provider.adapter_id != adapter.adapter_id {
            return Err(AgentError::invalid_input(format!(
                "adapter `{}` does not match provider instance `{}`",
                adapter.adapter_id, adapter.provider_instance_id
            )));
        }
    }

    unique_ids(
        profile
            .plugins
            .iter()
            .map(|plugin| plugin.plugin_id.as_str()),
        "plugin",
    )?;
    if profile.plugins.iter().any(|plugin| plugin.generation == 0) {
        return Err(AgentError::invalid_input(
            "profile plugin generation must be non-zero",
        ));
    }
    unique_ids(
        profile
            .prompt_fragments
            .iter()
            .map(|fragment| fragment.fragment_id.as_str()),
        "prompt fragment",
    )?;
    if profile
        .system_instructions
        .iter()
        .any(|instruction| instruction.trim().is_empty())
        || profile
            .prompt_fragments
            .iter()
            .any(|fragment| fragment.content.trim().is_empty())
    {
        return Err(AgentError::invalid_input(
            "system instructions and prompt fragments cannot be empty",
        ));
    }
    unique_ids(
        profile.context.provider_ids.iter().map(String::as_str),
        "context provider",
    )?;

    if profile
        .permissions
        .auto_allow
        .iter()
        .any(|effect| profile.permissions.require_approval.contains(effect))
    {
        return Err(AgentError::invalid_input(
            "a side-effect class cannot be both auto-allowed and approval-bound",
        ));
    }

    let persistence = &profile.persistence_distribution;
    if persistence.durable_sessions
        && (missing(&persistence.session_store_service) || missing(&persistence.database_service))
    {
        return Err(AgentError::invalid_input(
            "durable sessions require session-store and database services",
        ));
    }
    if (persistence.remote_adapters || persistence.remote_subagents)
        && missing(&persistence.distributed_service)
    {
        return Err(AgentError::invalid_input(
            "remote execution requires a distributed service",
        ));
    }
    Ok(())
}

pub fn reference_coding_agent_test_profile() -> AgentRuntimeProfile {
    AgentRuntimeProfileBuilder::new("mutsuki.reference.coding-agent")
        .mode(AgentRuntimeMode::Test)
        .provider(AgentProviderInstance {
            instance_id: "reference-provider-primary".into(),
            adapter_id: "openai-compatible".into(),
            credential_ref: None,
            capability_tags: vec![
                "streaming".into(),
                "tools".into(),
                "structured-output".into(),
            ],
            endpoint_profile: Some("provider-a".into()),
            test_only: true,
        })
        .provider(AgentProviderInstance {
            instance_id: "reference-provider-secondary".into(),
            adapter_id: "openai-compatible".into(),
            credential_ref: None,
            capability_tags: vec![
                "streaming".into(),
                "tools".into(),
                "structured-output".into(),
            ],
            endpoint_profile: Some("provider-b".into()),
            test_only: true,
        })
        .adapter(AgentProtocolAdapterSelection {
            protocol_family: "openai-compatible".into(),
            adapter_id: "openai-compatible".into(),
            provider_instance_id: "reference-provider-primary".into(),
            model: "reference-coding-model".into(),
            fallback_provider_instance_ids: vec!["reference-provider-secondary".into()],
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.context".into(),
            generation: 1,
            tools: Vec::new(),
            services: vec!["mutsuki.agent.context@1".into()],
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.loop".into(),
            generation: 1,
            tools: Vec::new(),
            services: Vec::new(),
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.model-gateway".into(),
            generation: 1,
            tools: Vec::new(),
            services: vec!["mutsuki.agent.model@1".into()],
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.tool-router".into(),
            generation: 1,
            tools: vec![
                "ask_user_question".into(),
                "confirm_plan".into(),
                "computer.fs.read".into(),
                "computer.fs.write".into(),
                "computer.fs.patch".into(),
                "computer.shell.exec".into(),
                "computer.browser.snapshot".into(),
                "git.status".into(),
                "git.diff".into(),
                "git.log".into(),
                "lsp.hover".into(),
                "lsp.workspace_symbols".into(),
                "code.search".into(),
                "code.symbol_query".into(),
                "mcp.call".into(),
            ],
            services: Vec::new(),
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.code-index".into(),
            generation: 1,
            tools: Vec::new(),
            services: vec![mutsuki_agent_contracts::AGENT_CODE_INDEX_PROTOCOL.into()],
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.next-edit".into(),
            generation: 1,
            tools: Vec::new(),
            services: vec![mutsuki_agent_contracts::AGENT_NEXT_EDIT_PROTOCOL.into()],
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.git".into(),
            generation: 1,
            tools: vec![
                "git.status".into(),
                "git.diff".into(),
                "git.stage".into(),
                "git.commit".into(),
                "git.push".into(),
            ],
            services: vec![mutsuki_agent_contracts::AGENT_GIT_PROTOCOL.into()],
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.lsp".into(),
            generation: 1,
            tools: vec!["lsp.hover".into(), "lsp.diagnostics".into()],
            services: vec![mutsuki_agent_contracts::AGENT_LSP_PROTOCOL.into()],
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.computer-use".into(),
            generation: 1,
            tools: vec![
                "computer.fs.read".into(),
                "computer.fs.patch".into(),
                "computer.shell.exec".into(),
            ],
            services: vec![mutsuki_agent_contracts::AGENT_COMPUTER_USE_PROTOCOL.into()],
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.mcp".into(),
            generation: 1,
            tools: vec!["mcp.call".into()],
            services: vec![mutsuki_agent_contracts::AGENT_MCP_PROTOCOL.into()],
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.session".into(),
            generation: 1,
            tools: Vec::new(),
            services: vec!["mutsuki.agent.session-store@1".into()],
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.skills".into(),
            generation: 1,
            tools: Vec::new(),
            services: vec!["mutsuki.agent.skills@1".into()],
        })
        .plugin(AgentProfilePlugin {
            plugin_id: "mutsuki.plugin.agent.knowledge".into(),
            generation: 1,
            tools: Vec::new(),
            services: vec!["mutsuki.agent.knowledge@1".into()],
        })
        .system_instruction(
            "Act on the product-provided workspace and return verifiable coding results.",
        )
        .prompt_fragment(AgentPromptFragment {
            fragment_id: "reference-coding-safety".into(),
            content: "Use read-only tools freely and request approval before workspace writes."
                .into(),
            priority: 0,
        })
        .context_policy(AgentContextPolicy {
            provider_ids: vec![
                "workspace".into(),
                "git".into(),
                "lsp".into(),
                "mutsuki.agent.context.code-index".into(),
                "mutsuki.agent.context.next-edit".into(),
                "mutsuki.agent.context.computer-use".into(),
                "mutsuki.agent.context.mcp".into(),
                "editor".into(),
            ],
            budget: mutsuki_agent_contracts::ContextBudget {
                max_tokens: Some(32_000),
                max_bytes: Some(2 * 1024 * 1024),
                max_items: Some(512),
            },
            compaction_service: Some("mutsuki.agent.context.compaction@1".into()),
            provider_options: serde_json::json!({}),
        })
        .permission_policy(AgentPermissionPolicy {
            auto_allow: vec![
                mutsuki_agent_contracts::ToolSideEffect::None,
                mutsuki_agent_contracts::ToolSideEffect::WorkspaceRead,
            ],
            require_approval: vec![
                mutsuki_agent_contracts::ToolSideEffect::WorkspaceWrite,
                mutsuki_agent_contracts::ToolSideEffect::ExternalWrite,
            ],
            allowed_permissions: vec![
                "workspace.read".into(),
                "workspace.write".into(),
                "git.read".into(),
                "git.write".into(),
                "git.high_risk".into(),
                "lsp.read".into(),
                "code.index.read".into(),
                "next.edit.read".into(),
                "mcp.call".into(),
                "subagent.delegate".into(),
            ],
        })
        .budget_policy(AgentBudget {
            max_steps: Some(64),
            max_total_tokens: Some(1_000_000),
            max_cost_microunits: Some(10_000_000),
            deadline_unix_ms: None,
        })
        .persistence_distribution(AgentPersistenceDistributionPolicy {
            durable_sessions: true,
            session_store_service: Some("mutsuki.agent.session-store@1".into()),
            database_service: Some("mutsuki.database@1".into()),
            distributed_service: Some("mutsuki.distributed@1".into()),
            remote_adapters: true,
            remote_subagents: true,
        })
        .build()
        .expect("reference coding-agent test profile is valid")
}

fn unique_ids<'a>(
    ids: impl IntoIterator<Item = &'a str>,
    label: &str,
) -> Result<BTreeSet<&'a str>, AgentError> {
    let mut unique = BTreeSet::new();
    for id in ids {
        require_id(id, label)?;
        if !unique.insert(id) {
            return Err(AgentError::invalid_input(format!(
                "duplicate {label} `{id}`",
            )));
        }
    }
    Ok(unique)
}

fn require_id(value: &str, label: &str) -> Result<(), AgentError> {
    if value.trim().is_empty() {
        Err(AgentError::invalid_input(format!("{label} is required")))
    } else {
        Ok(())
    }
}

fn missing(value: &Option<String>) -> bool {
    value.as_deref().is_none_or(|value| value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use mutsuki_agent_contracts::{ToolSideEffect, *};

    use super::*;

    fn provider(test_only: bool) -> AgentProviderInstance {
        AgentProviderInstance {
            instance_id: "primary".into(),
            adapter_id: "openai-compatible".into(),
            credential_ref: None,
            capability_tags: vec!["tools".into(), "structured-output".into()],
            endpoint_profile: Some("product-owned-endpoint".into()),
            test_only,
        }
    }

    #[test]
    fn profile_composes_all_public_policy_surfaces() {
        let profile = AgentRuntimeProfileBuilder::new("coding")
            .mode(AgentRuntimeMode::Development)
            .provider(provider(false))
            .adapter(AgentProtocolAdapterSelection {
                protocol_family: "openai-compatible".into(),
                adapter_id: "openai-compatible".into(),
                provider_instance_id: "primary".into(),
                model: "coding-model".into(),
                fallback_provider_instance_ids: Vec::new(),
            })
            .plugin(AgentProfilePlugin {
                plugin_id: "mutsuki.plugin.agent.lsp".into(),
                generation: 7,
                tools: vec!["lsp.hover".into()],
                services: vec!["mutsuki.agent.lsp@1".into()],
            })
            .system_instruction("Operate on the product-provided workspace context.")
            .prompt_fragment(AgentPromptFragment {
                fragment_id: "coding-safety".into(),
                content: "Request approval before writes.".into(),
                priority: 0,
            })
            .context_policy(AgentContextPolicy {
                provider_ids: vec!["workspace".into(), "lsp".into()],
                budget: ContextBudget {
                    max_tokens: Some(32_000),
                    max_bytes: Some(1_000_000),
                    max_items: Some(256),
                },
                compaction_service: Some("mutsuki.agent.compaction@1".into()),
                provider_options: serde_json::json!({}),
            })
            .permission_policy(AgentPermissionPolicy {
                auto_allow: vec![ToolSideEffect::None, ToolSideEffect::WorkspaceRead],
                require_approval: vec![ToolSideEffect::WorkspaceWrite],
                allowed_permissions: vec!["workspace.read".into(), "workspace.write".into()],
            })
            .budget_policy(AgentBudget {
                max_steps: Some(64),
                max_total_tokens: Some(1_000_000),
                max_cost_microunits: Some(5_000_000),
                deadline_unix_ms: None,
            })
            .persistence_distribution(AgentPersistenceDistributionPolicy {
                durable_sessions: true,
                session_store_service: Some("mutsuki.agent.session-store@1".into()),
                database_service: Some("mutsuki.database@1".into()),
                distributed_service: Some("mutsuki.distributed@1".into()),
                remote_adapters: true,
                remote_subagents: true,
            })
            .build()
            .unwrap();
        assert_eq!(profile.providers.len(), 1);
        assert_eq!(profile.plugins[0].generation, 7);
        assert!(profile.persistence_distribution.durable_sessions);
    }

    #[test]
    fn production_profile_rejects_test_provider() {
        let error = AgentRuntimeProfileBuilder::new("production")
            .provider(provider(true))
            .adapter(AgentProtocolAdapterSelection {
                protocol_family: "openai-compatible".into(),
                adapter_id: "openai-compatible".into(),
                provider_instance_id: "primary".into(),
                model: "model".into(),
                fallback_provider_instance_ids: Vec::new(),
            })
            .build()
            .unwrap_err();
        assert_eq!(error.code, "agent.profile.test_provider_in_production");
    }

    #[test]
    fn reference_profile_is_complete_and_explicitly_test_only() {
        let profile = reference_coding_agent_test_profile();
        assert_eq!(profile.mode, AgentRuntimeMode::Test);
        assert!(profile.providers.iter().all(|provider| provider.test_only));
        assert_eq!(profile.providers.len(), 2);
        assert_eq!(profile.adapters.len(), 1);
        assert_eq!(
            profile.adapters[0].fallback_provider_instance_ids,
            vec!["reference-provider-secondary".to_string()]
        );
        assert!(profile.persistence_distribution.durable_sessions);
        assert!(profile.persistence_distribution.remote_subagents);
        assert!(profile.plugins.iter().any(|plugin| {
            plugin
                .tools
                .iter()
                .any(|tool| tool == "computer.fs.patch" || tool == "computer.fs.write")
        }));
        assert!(
            profile
                .plugins
                .iter()
                .any(|plugin| plugin.plugin_id == "mutsuki.plugin.agent.git")
        );
        assert!(
            profile
                .plugins
                .iter()
                .any(|plugin| plugin.plugin_id == "mutsuki.plugin.agent.computer-use")
        );
    }
}
