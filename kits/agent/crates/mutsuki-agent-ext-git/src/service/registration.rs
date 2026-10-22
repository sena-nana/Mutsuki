//! Plugin manifest registration and tool descriptors for the Git service.

use mutsuki_agent_contracts::{
    AgentError, AgentPluginStateKind, AgentServiceDescriptor, AgentToolDescriptor, GitRisk,
};
use mutsuki_agent_ext_api::{AgentPluginRegistrar, ToolProvider};

use super::SharedGitService;
use crate::{CONTEXT_PROVIDER_ID, PLUGIN_ID, SERVICE_ID};

impl SharedGitService {
    pub fn plugin_descriptor(
        generation: u64,
    ) -> Result<mutsuki_agent_contracts::AgentKitPluginDescriptor, AgentError> {
        let mut registrar = AgentPluginRegistrar::new(PLUGIN_ID, generation)
            .service(AgentServiceDescriptor {
                service_id: SERVICE_ID.into(),
                version: "1".into(),
                request_schema: "mutsuki.agent.git.request@1".into(),
                response_schema: "mutsuki.agent.git.response@1".into(),
                state: AgentPluginStateKind::Stateful,
                affinity: Some("worktree".into()),
            })
            .context_provider(CONTEXT_PROVIDER_ID)
            .require_capability("workspace.process")
            .require_service(SERVICE_ID);
        for (name, risk) in [
            ("git.status", GitRisk::ReadOnly),
            ("git.diff", GitRisk::ReadOnly),
            ("git.log", GitRisk::ReadOnly),
            ("git.show", GitRisk::ReadOnly),
            ("git.blame", GitRisk::ReadOnly),
            ("git.branch.list", GitRisk::ReadOnly),
            ("git.worktree.list", GitRisk::ReadOnly),
            ("git.stage", GitRisk::ReversibleWrite),
            ("git.unstage", GitRisk::ReversibleWrite),
            ("git.commit", GitRisk::ReversibleWrite),
            ("git.branch.create", GitRisk::ReversibleWrite),
            ("git.branch.switch", GitRisk::ReversibleWrite),
            ("git.push", GitRisk::HighRisk),
            ("git.pull", GitRisk::HighRisk),
            ("git.fetch", GitRisk::HighRisk),
            ("git.merge", GitRisk::HighRisk),
            ("git.rebase", GitRisk::HighRisk),
            ("git.reset", GitRisk::HighRisk),
            ("git.cherry_pick", GitRisk::HighRisk),
            ("git.branch.delete", GitRisk::HighRisk),
            ("git.worktree.create", GitRisk::HighRisk),
            ("git.worktree.remove", GitRisk::HighRisk),
        ] {
            let mut tool = AgentToolDescriptor::new(
                name,
                format!("mutsuki.agent.tool.{name}@1"),
                format!("Run {name}"),
            );
            tool.side_effect = risk.side_effect();
            tool.requires_approval = risk.requires_approval();
            tool.permissions = match risk {
                GitRisk::ReadOnly => vec!["git.read".into()],
                GitRisk::ReversibleWrite => vec!["git.write".into()],
                GitRisk::HighRisk => vec!["git.high_risk".into()],
            };
            registrar = registrar.tool(tool);
        }
        registrar.build()
    }
}

impl ToolProvider for SharedGitService {
    fn tools(&self) -> Vec<AgentToolDescriptor> {
        SharedGitService::plugin_descriptor(1)
            .map(|descriptor| descriptor.tools)
            .unwrap_or_default()
    }
}
