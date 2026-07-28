use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ResourceRef;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSourceKind {
    Sandbox = 10,
    Plugin = 20,
    User = 30,
    Workspace = 40,
}

impl SkillSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Plugin => "plugin",
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }

    pub fn priority(self) -> u8 {
        self as u8
    }

    pub fn trusted_by_default(self) -> bool {
        matches!(self, Self::Workspace | Self::Plugin)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillProvenance {
    pub source_kind: SkillSourceKind,
    pub source_path: String,
    pub package_hash: String,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillCompatibility {
    #[serde(default)]
    pub agentkit_api: Option<String>,
    #[serde(default)]
    pub min_runtime: Option<String>,
}

impl Default for SkillCompatibility {
    fn default() -> Self {
        Self {
            agentkit_api: None,
            min_runtime: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSecurityMetadata {
    #[serde(default)]
    pub trusted: bool,
    #[serde(default)]
    pub allow_scripts: bool,
    #[serde(default)]
    pub external: bool,
}

impl Default for SkillSecurityMetadata {
    fn default() -> Self {
        Self {
            trusted: false,
            allow_scripts: false,
            external: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillDescriptor {
    pub skill_id: String,
    pub version: String,
    pub title: String,
    pub summary: String,
    pub instructions: ResourceRef,
    #[serde(default)]
    pub required_tools: Vec<String>,
    #[serde(default)]
    pub optional_tools: Vec<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub required_services: Vec<String>,
    #[serde(default)]
    pub bundled_resources: Vec<ResourceRef>,
    #[serde(default)]
    pub compatibility: SkillCompatibility,
    #[serde(default)]
    pub security: SkillSecurityMetadata,
    pub provenance: SkillProvenance,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillCatalogEntry {
    pub skill_id: String,
    pub version: String,
    pub title: String,
    pub summary: String,
    pub source_kind: SkillSourceKind,
    pub available: bool,
    #[serde(default)]
    pub unavailable_reasons: Vec<String>,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillOverrideEvent {
    pub skill_id: String,
    pub winner_source: SkillSourceKind,
    pub winner_version: String,
    pub overridden_source: SkillSourceKind,
    pub overridden_version: String,
    pub compatible: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDiscoverRequest {
    #[serde(default)]
    pub include_unavailable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillDiscoverResult {
    pub catalog: Vec<SkillCatalogEntry>,
    #[serde(default)]
    pub overrides: Vec<SkillOverrideEvent>,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLoadRequest {
    pub skill_id: String,
    #[serde(default)]
    pub generation: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillLoadResult {
    pub descriptor: SkillDescriptor,
    pub instructions_text: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillReloadRequest {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillReloadResult {
    pub previous_generation: u64,
    pub generation: u64,
    pub discovered: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSkillPolicy {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allow_script_resources: bool,
    /// Sources treated as trusted for side-effect tools. Empty → workspace + plugin.
    #[serde(default)]
    pub trusted_sources: Vec<SkillSourceKind>,
}

impl Default for AgentSkillPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_script_resources: false,
            trusted_sources: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}
