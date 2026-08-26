//! Tool routing owned by the Runtime domain.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentError, AgentResult, AgentRuntimeProfile, AgentToolDescriptor, AgentToolListRequest,
    AgentToolListResult,
};

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Arc<Mutex<BTreeMap<String, AgentToolDescriptor>>>,
    profile_allowlists: Arc<Mutex<BTreeMap<String, BTreeSet<String>>>>,
}

impl ToolRegistry {
    pub fn register(&self, descriptor: AgentToolDescriptor) -> AgentResult<()> {
        if descriptor.name.trim().is_empty() {
            return Err(AgentError::invalid_input("tool name is required"));
        }
        if descriptor.target_protocol_id.trim().is_empty() {
            return Err(AgentError::invalid_input("target_protocol_id is required"));
        }
        self.tools
            .lock()
            .expect("tool registry mutex poisoned")
            .insert(descriptor.name.clone(), descriptor);
        Ok(())
    }

    pub fn configure_profile(&self, profile: &AgentRuntimeProfile) {
        let allowlist = profile
            .plugins
            .iter()
            .flat_map(|plugin| plugin.tools.iter().cloned())
            .collect::<BTreeSet<_>>();
        let mut allowlists = self
            .profile_allowlists
            .lock()
            .expect("tool registry mutex poisoned");
        if allowlist.is_empty() {
            allowlists.remove(&profile.profile_id);
        } else {
            allowlists.insert(profile.profile_id.clone(), allowlist);
        }
    }

    pub fn list(&self, request: AgentToolListRequest) -> AgentToolListResult {
        let tools = self
            .tools
            .lock()
            .expect("tool registry mutex poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let tools = match request.profile_id.as_deref() {
            Some(profile_id) => {
                let allowlists = self
                    .profile_allowlists
                    .lock()
                    .expect("tool registry mutex poisoned");
                match allowlists.get(profile_id) {
                    Some(allowlist) => tools
                        .into_iter()
                        .filter(|tool| allowlist.contains(&tool.name))
                        .collect(),
                    None => tools,
                }
            }
            None => tools,
        };
        AgentToolListResult { tools }
    }

    pub fn get(&self, name: &str) -> AgentResult<AgentToolDescriptor> {
        self.tools
            .lock()
            .expect("tool registry mutex poisoned")
            .get(name)
            .cloned()
            .ok_or_else(|| AgentError::not_found(format!("tool `{name}` not registered")))
    }
}

#[cfg(test)]
mod tests {
    use mutsuki_agent_contracts::{AgentProfilePlugin, AgentToolDescriptor, AgentToolListRequest};

    use super::ToolRegistry;
    use crate::AgentRuntimeProfileBuilder;

    #[test]
    fn list_filters_registered_tools_by_profile_allowlist() {
        let registry = ToolRegistry::default();
        registry
            .register(AgentToolDescriptor::new(
                "git.status",
                "mutsuki.agent.git/call@1",
                "status",
            ))
            .unwrap();
        registry
            .register(AgentToolDescriptor::new(
                "shell.exec",
                "mutsuki.agent.shell/call@1",
                "exec",
            ))
            .unwrap();
        let profile = AgentRuntimeProfileBuilder::new("coding")
            .plugin(AgentProfilePlugin {
                plugin_id: "git".into(),
                generation: 1,
                tools: vec!["git.status".into()],
                services: Vec::new(),
            })
            .build()
            .unwrap();
        registry.configure_profile(&profile);
        let listed = registry.list(AgentToolListRequest {
            profile_id: Some("coding".into()),
        });
        assert_eq!(listed.tools.len(), 1);
        assert_eq!(listed.tools[0].name, "git.status");
        assert_eq!(
            registry.list(AgentToolListRequest::default()).tools.len(),
            2
        );
    }
}
