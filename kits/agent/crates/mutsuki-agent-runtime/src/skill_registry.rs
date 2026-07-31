//! Skill discovery, override resolution, and on-demand loading.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentError, AgentResult, AgentSkillPolicy, SkillCatalogEntry, SkillCompatibility,
    SkillDescriptor, SkillDiscoverRequest, SkillDiscoverResult, SkillLoadRequest, SkillLoadResult,
    SkillOverrideEvent, SkillProvenance, SkillReloadRequest, SkillReloadResult,
    SkillSecurityMetadata, SkillSourceKind, ToolSideEffect,
};
use mutsuki_agent_sdk::skill_resource_ref;
use sha2::{Digest, Sha256};

use crate::tool_routing::ToolRegistry;

const SKILL_OWNER_ID: &str = "mutsuki.agent.runtime.skills";

const SCRIPT_EXTENSIONS: &[&str] = &[".sh", ".py", ".js"];

#[derive(Clone, Debug, Default)]
pub struct SkillRoots {
    pub workspace: Option<PathBuf>,
    pub plugin: Option<PathBuf>,
    pub user: Option<PathBuf>,
    pub sandbox: Option<PathBuf>,
}

#[derive(Clone)]
pub struct SkillRegistry {
    inner: Arc<SkillRegistryInner>,
}

struct SkillRegistryInner {
    generation: AtomicU64,
    policy: Mutex<AgentSkillPolicy>,
    roots: Mutex<SkillRoots>,
    snapshots: Mutex<BTreeMap<u64, SkillSnapshot>>,
    tool_registry: Mutex<Option<ToolRegistry>>,
}

#[derive(Clone, Debug)]
struct SkillSnapshot {
    packages: BTreeMap<String, ResolvedSkillPackage>,
    overrides: Vec<SkillOverrideEvent>,
}

#[derive(Clone, Debug)]
struct ResolvedSkillPackage {
    descriptor: SkillDescriptor,
    instructions_text: String,
}

#[derive(Clone, Debug)]
struct ParsedSkillPackage {
    skill_id: String,
    version: String,
    title: String,
    summary: String,
    instructions_text: String,
    required_tools: Vec<String>,
    optional_tools: Vec<String>,
    required_capabilities: Vec<String>,
    required_services: Vec<String>,
    bundled_resource_names: Vec<String>,
    package_dir: PathBuf,
    source_kind: SkillSourceKind,
    source_path: String,
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new(AgentSkillPolicy::default(), SkillRoots::default())
    }
}

impl SkillRegistry {
    pub fn new(policy: AgentSkillPolicy, roots: SkillRoots) -> Self {
        let registry = Self {
            inner: Arc::new(SkillRegistryInner {
                generation: AtomicU64::new(0),
                policy: Mutex::new(policy),
                roots: Mutex::new(roots),
                snapshots: Mutex::new(BTreeMap::new()),
                tool_registry: Mutex::new(None),
            }),
        };
        let _ = registry.reload(SkillReloadRequest {});
        registry
    }

    pub fn with_tool_registry(self, tool_registry: ToolRegistry) -> Self {
        *self
            .inner
            .tool_registry
            .lock()
            .expect("skill registry mutex poisoned") = Some(tool_registry);
        self
    }

    pub fn set_roots(&self, roots: SkillRoots) {
        *self
            .inner
            .roots
            .lock()
            .expect("skill registry mutex poisoned") = roots;
    }

    pub fn set_policy(&self, policy: AgentSkillPolicy) {
        *self
            .inner
            .policy
            .lock()
            .expect("skill registry mutex poisoned") = policy;
    }

    pub fn discover(&self, request: SkillDiscoverRequest) -> AgentResult<SkillDiscoverResult> {
        let generation = self.current_generation()?;
        let snapshot = self.snapshot_for_generation(generation)?;
        let catalog = snapshot
            .packages
            .values()
            .filter_map(|pkg| {
                let entry = catalog_entry_from_package(&pkg.descriptor, generation);
                (request.include_unavailable || entry.available).then_some(entry)
            })
            .collect();
        Ok(SkillDiscoverResult {
            catalog,
            overrides: snapshot.overrides.clone(),
            generation,
        })
    }

    pub fn load(&self, request: SkillLoadRequest) -> AgentResult<SkillLoadResult> {
        let generation = request
            .generation
            .unwrap_or_else(|| self.current_generation().unwrap_or(0));
        let snapshot = self.snapshot_for_generation(generation)?;
        let pkg = snapshot.packages.get(&request.skill_id).ok_or_else(|| {
            AgentError::not_found(format!("skill `{}` not found", request.skill_id))
        })?;
        let entry = catalog_entry_from_package(&pkg.descriptor, generation);
        if !entry.available {
            return Err(AgentError::provider_unavailable(format!(
                "skill `{}` is unavailable: {}",
                request.skill_id,
                entry.unavailable_reasons.join("; ")
            )));
        }
        Ok(SkillLoadResult {
            descriptor: pkg.descriptor.clone(),
            instructions_text: pkg.instructions_text.clone(),
            generation,
        })
    }

    pub fn reload(&self, _request: SkillReloadRequest) -> AgentResult<SkillReloadResult> {
        let previous_generation = self.inner.generation.load(Ordering::SeqCst);
        let generation = previous_generation.saturating_add(1).max(1);
        let snapshot = self.scan_and_resolve(generation)?;
        let discovered = snapshot.packages.len();
        self.inner
            .snapshots
            .lock()
            .expect("skill registry mutex poisoned")
            .insert(generation, snapshot);
        self.inner.generation.store(generation, Ordering::SeqCst);
        Ok(SkillReloadResult {
            previous_generation,
            generation,
            discovered,
        })
    }

    fn current_generation(&self) -> AgentResult<u64> {
        let generation = self.inner.generation.load(Ordering::SeqCst);
        if generation == 0 {
            return Err(AgentError::provider_unavailable(
                "skill registry has not been loaded",
            ));
        }
        Ok(generation)
    }

    fn snapshot_for_generation(&self, generation: u64) -> AgentResult<SkillSnapshot> {
        self.inner
            .snapshots
            .lock()
            .expect("skill registry mutex poisoned")
            .get(&generation)
            .cloned()
            .ok_or_else(|| {
                AgentError::not_found(format!("skill generation `{generation}` not available"))
            })
    }

    fn scan_and_resolve(&self, generation: u64) -> AgentResult<SkillSnapshot> {
        let policy = self
            .inner
            .policy
            .lock()
            .expect("skill registry mutex poisoned")
            .clone();
        if !policy.enabled {
            return Ok(SkillSnapshot {
                packages: BTreeMap::new(),
                overrides: Vec::new(),
            });
        }
        let roots = self
            .inner
            .roots
            .lock()
            .expect("skill registry mutex poisoned")
            .clone();
        let tool_registry = self
            .inner
            .tool_registry
            .lock()
            .expect("skill registry mutex poisoned")
            .clone();

        let mut by_id: HashMap<String, Vec<ParsedSkillPackage>> = HashMap::new();
        for (source_kind, root) in roots.entries() {
            let Some(root) = root else { continue };
            if !root.is_dir() {
                continue;
            }
            for entry in discover_skill_dirs(&root)? {
                let parsed = parse_skill_package(&entry, source_kind)?;
                by_id
                    .entry(parsed.skill_id.clone())
                    .or_default()
                    .push(parsed);
            }
        }

        let mut overrides = Vec::new();
        let mut packages = BTreeMap::new();
        for (skill_id, mut candidates) in by_id {
            candidates.sort_by(|left, right| {
                right
                    .source_kind
                    .priority()
                    .cmp(&left.source_kind.priority())
                    .then_with(|| left.source_path.cmp(&right.source_path))
            });
            let winner = candidates.remove(0);
            for overridden in candidates {
                let compatible =
                    major_version(&overridden.version) == major_version(&winner.version);
                overrides.push(SkillOverrideEvent {
                    skill_id: skill_id.clone(),
                    winner_source: winner.source_kind,
                    winner_version: winner.version.clone(),
                    overridden_source: overridden.source_kind,
                    overridden_version: overridden.version.clone(),
                    compatible,
                    detail: if compatible {
                        format!(
                            "{} ({}) overrides {} ({})",
                            winner.source_kind.as_str(),
                            winner.version,
                            overridden.source_kind.as_str(),
                            overridden.version
                        )
                    } else {
                        format!(
                            "incompatible major version: {} ({}) refused silent merge with {} ({})",
                            overridden.source_kind.as_str(),
                            overridden.version,
                            winner.source_kind.as_str(),
                            winner.version
                        )
                    },
                });
            }
            let (descriptor, instructions_text) =
                finalize_package(winner, generation, &policy, tool_registry.as_ref())?;
            packages.insert(
                skill_id,
                ResolvedSkillPackage {
                    descriptor,
                    instructions_text,
                },
            );
        }
        Ok(SkillSnapshot {
            packages,
            overrides,
        })
    }
}

impl SkillRoots {
    fn entries(&self) -> [(SkillSourceKind, Option<PathBuf>); 4] {
        [
            (SkillSourceKind::Sandbox, self.sandbox.clone()),
            (SkillSourceKind::Plugin, self.plugin.clone()),
            (SkillSourceKind::User, self.user.clone()),
            (SkillSourceKind::Workspace, self.workspace.clone()),
        ]
    }
}

fn discover_skill_dirs(root: &Path) -> AgentResult<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    let entries = fs::read_dir(root).map_err(|error| {
        AgentError::provider_unavailable(format!(
            "failed to read skill root `{}`: {error}",
            root.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            AgentError::provider_unavailable(format!("skill root read failed: {error}"))
        })?;
        let path = entry.path();
        if path.is_dir() && path.join("SKILL.md").is_file() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn parse_skill_package(
    package_dir: &Path,
    source_kind: SkillSourceKind,
) -> AgentResult<ParsedSkillPackage> {
    let skill_md_path = package_dir.join("SKILL.md");
    let raw = fs::read_to_string(&skill_md_path).map_err(|error| {
        AgentError::invalid_input(format!(
            "failed to read `{}`: {error}",
            skill_md_path.display()
        ))
    })?;
    let (frontmatter, instructions_text) = split_frontmatter(&raw)?;
    let fields = parse_frontmatter(&frontmatter);
    let skill_id = fields
        .get("id")
        .or_else(|| fields.get("name"))
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AgentError::invalid_input("skill id or name is required"))?;
    let version = fields
        .get("version")
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "0.1.0".into());
    let title = fields
        .get("title")
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| skill_id.clone());
    let summary = fields
        .get("summary")
        .or_else(|| fields.get("description"))
        .cloned()
        .unwrap_or_default();
    Ok(ParsedSkillPackage {
        skill_id,
        version,
        title,
        summary,
        instructions_text: instructions_text.trim().to_string(),
        required_tools: parse_list_field(&fields, "required_tools"),
        optional_tools: parse_list_field(&fields, "optional_tools"),
        required_capabilities: parse_list_field(&fields, "required_capabilities"),
        required_services: parse_list_field(&fields, "required_services"),
        bundled_resource_names: parse_list_field(&fields, "resources"),
        package_dir: package_dir.to_path_buf(),
        source_kind,
        source_path: package_dir.display().to_string(),
    })
}

fn finalize_package(
    parsed: ParsedSkillPackage,
    generation: u64,
    policy: &AgentSkillPolicy,
    tool_registry: Option<&ToolRegistry>,
) -> AgentResult<(SkillDescriptor, String)> {
    let trusted = is_trusted_source(parsed.source_kind, policy);
    let allow_scripts = policy.allow_script_resources;
    let mut bundled_resources = Vec::new();
    let mut unavailable_reasons = Vec::new();

    for resource_name in &parsed.bundled_resource_names {
        let confined = match confine_package_path(&parsed.package_dir, resource_name) {
            Ok(path) => path,
            Err(error) => {
                unavailable_reasons.push(error.message);
                continue;
            }
        };
        if is_script_path(&confined) && !allow_scripts {
            unavailable_reasons.push(format!(
                "script resource `{}` rejected by policy",
                resource_name
            ));
            continue;
        }
        if !confined.is_file() {
            unavailable_reasons.push(format!("bundled resource `{resource_name}` not found"));
            continue;
        }
        bundled_resources.push(skill_resource_ref(
            SKILL_OWNER_ID,
            &parsed.skill_id,
            resource_name,
        ));
    }

    if let Some(registry) = tool_registry {
        for tool_name in &parsed.required_tools {
            match registry.get(tool_name) {
                Ok(descriptor) => {
                    if !trusted && descriptor.side_effect != ToolSideEffect::None {
                        unavailable_reasons.push(format!(
                            "required tool `{tool_name}` has side effects and source is untrusted"
                        ));
                    }
                }
                Err(_) => unavailable_reasons.push(format!("required tool `{tool_name}` missing")),
            }
        }
    }

    for capability in &parsed.required_capabilities {
        if capability.trim().is_empty() {
            unavailable_reasons.push("required capability cannot be empty".into());
        }
    }

    let package_hash = compute_package_hash(&parsed)?;
    let instructions = skill_resource_ref(SKILL_OWNER_ID, &parsed.skill_id, "instructions");
    let security = SkillSecurityMetadata {
        trusted,
        allow_scripts,
        external: matches!(
            parsed.source_kind,
            SkillSourceKind::User | SkillSourceKind::Sandbox
        ),
    };
    let descriptor = SkillDescriptor {
        skill_id: parsed.skill_id.clone(),
        version: parsed.version.clone(),
        title: parsed.title.clone(),
        summary: parsed.summary.clone(),
        instructions: instructions.clone(),
        required_tools: parsed.required_tools.clone(),
        optional_tools: parsed.optional_tools.clone(),
        required_capabilities: parsed.required_capabilities.clone(),
        required_services: parsed.required_services.clone(),
        bundled_resources,
        compatibility: SkillCompatibility::default(),
        security,
        provenance: SkillProvenance {
            source_kind: parsed.source_kind,
            source_path: parsed.source_path.clone(),
            package_hash,
            generation,
        },
        metadata: serde_json::json!({
            "available": unavailable_reasons.is_empty(),
            "unavailable_reasons": unavailable_reasons,
        }),
    };
    Ok((descriptor, parsed.instructions_text))
}

fn catalog_entry_from_package(descriptor: &SkillDescriptor, generation: u64) -> SkillCatalogEntry {
    let metadata = &descriptor.metadata;
    let available = metadata
        .get("available")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let unavailable_reasons = metadata
        .get("unavailable_reasons")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    SkillCatalogEntry {
        skill_id: descriptor.skill_id.clone(),
        version: descriptor.version.clone(),
        title: descriptor.title.clone(),
        summary: descriptor.summary.clone(),
        source_kind: descriptor.provenance.source_kind,
        available,
        unavailable_reasons,
        generation,
    }
}

fn is_trusted_source(source_kind: SkillSourceKind, policy: &AgentSkillPolicy) -> bool {
    if !policy.trusted_sources.is_empty() {
        return policy.trusted_sources.contains(&source_kind);
    }
    source_kind.trusted_by_default()
}

fn major_version(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

fn split_frontmatter(raw: &str) -> AgentResult<(String, String)> {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        return Ok((String::new(), raw.to_string()));
    }
    let rest = trimmed.trim_start_matches("---").trim_start_matches('\n');
    let Some((frontmatter, body)) = rest.split_once("\n---") else {
        return Err(AgentError::invalid_input(
            "SKILL.md frontmatter is not terminated",
        ));
    };
    Ok((
        frontmatter.to_string(),
        body.trim_start_matches('\n').to_string(),
    ))
}

fn parse_frontmatter(frontmatter: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut current_key: Option<String> = None;
    let mut list_values: Vec<String> = Vec::new();

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if let Some(list_value) = trimmed.strip_prefix("- ") {
            if let Some(key) = &current_key {
                list_values.push(list_value.trim().to_string());
                fields.insert(key.clone(), list_values.join("\n"));
            }
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            if let Some(prev) = current_key.take()
                && !list_values.is_empty()
            {
                fields.insert(prev, list_values.join("\n"));
                list_values.clear();
            }
            let key = key.trim().to_string();
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if value.is_empty() {
                current_key = Some(key);
                list_values.clear();
            } else {
                fields.insert(key, value);
                current_key = None;
            }
        }
    }
    if let Some(prev) = current_key {
        fields.insert(prev, list_values.join("\n"));
    }
    fields
}

fn parse_list_field(fields: &BTreeMap<String, String>, key: &str) -> Vec<String> {
    fields
        .get(key)
        .map(|value| {
            value
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn compute_package_hash(parsed: &ParsedSkillPackage) -> AgentResult<String> {
    let mut hasher = Sha256::new();
    let skill_md = parsed.package_dir.join("SKILL.md");
    hasher.update(
        fs::read(&skill_md).map_err(|error| {
            AgentError::invalid_input(format!("failed to hash SKILL.md: {error}"))
        })?,
    );
    for resource in &parsed.bundled_resource_names {
        let Ok(path) = confine_package_path(&parsed.package_dir, resource) else {
            continue;
        };
        if path.is_file() {
            hasher.update(fs::read(&path).map_err(|error| {
                AgentError::invalid_input(format!("failed to hash `{resource}`: {error}"))
            })?);
        }
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub fn confine_package_path(package_dir: &Path, relative: &str) -> AgentResult<PathBuf> {
    if relative.contains('\\') {
        return Err(AgentError::invalid_input(format!(
            "resource path `{relative}` must use forward slashes"
        )));
    }
    let mut candidate = package_dir.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => candidate.push(part),
            Component::CurDir => {}
            _ => {
                return Err(AgentError::invalid_input(format!(
                    "resource path `{relative}` escapes package directory"
                )));
            }
        }
    }
    if !candidate.starts_with(package_dir) {
        return Err(AgentError::invalid_input(format!(
            "resource path `{relative}` escapes package directory"
        )));
    }
    Ok(candidate)
}

fn is_script_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            SCRIPT_EXTENSIONS
                .iter()
                .any(|suffix| ext.eq_ignore_ascii_case(suffix.trim_start_matches('.')))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::AgentToolDescriptor;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_skill(
        root: &Path,
        dir_name: &str,
        frontmatter: &str,
        body: &str,
        extra_files: &[(&str, &str)],
    ) -> PathBuf {
        let dir = root.join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        let mut file = fs::File::create(dir.join("SKILL.md")).unwrap();
        write!(file, "---\n{frontmatter}\n---\n\n{body}\n").unwrap();
        for (name, contents) in extra_files {
            if name.contains('/')
                && let Some(parent) = dir.join(name).parent()
            {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(dir.join(name), contents).unwrap();
        }
        dir
    }

    fn registry_with_roots(roots: SkillRoots) -> SkillRegistry {
        SkillRegistry::new(AgentSkillPolicy::default(), roots)
    }

    #[test]
    fn discovers_workspace_plugin_and_user_skills() {
        let workspace = TempDir::new().unwrap();
        let plugin = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        write_skill(
            workspace.path(),
            "ws-skill",
            "id: ws-skill\nversion: 1.0.0\ntitle: Workspace\nsummary: from workspace",
            "Do workspace things.",
            &[],
        );
        write_skill(
            plugin.path(),
            "plugin-skill",
            "id: plugin-skill\nversion: 1.0.0\ntitle: Plugin\nsummary: from plugin",
            "Do plugin things.",
            &[],
        );
        write_skill(
            user.path(),
            "user-skill",
            "id: user-skill\nversion: 1.0.0\ntitle: User\nsummary: from user",
            "Do user things.",
            &[],
        );
        let registry = registry_with_roots(SkillRoots {
            workspace: Some(workspace.path().to_path_buf()),
            plugin: Some(plugin.path().to_path_buf()),
            user: Some(user.path().to_path_buf()),
            sandbox: None,
        });
        let result = registry.discover(SkillDiscoverRequest::default()).unwrap();
        let ids: Vec<_> = result
            .catalog
            .iter()
            .map(|entry| entry.skill_id.as_str())
            .collect();
        assert!(ids.contains(&"ws-skill"));
        assert!(ids.contains(&"plugin-skill"));
        assert!(ids.contains(&"user-skill"));
    }

    #[test]
    fn workspace_overrides_user_plugin_and_sandbox() {
        let workspace = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let plugin = TempDir::new().unwrap();
        let sandbox = TempDir::new().unwrap();
        for (root, source, version) in [
            (sandbox.path(), "sandbox", "1.0.0"),
            (plugin.path(), "plugin", "1.0.0"),
            (user.path(), "user", "1.0.0"),
            (workspace.path(), "workspace", "2.0.0"),
        ] {
            write_skill(
                root,
                "dup-skill",
                &format!("id: dup-skill\nversion: {version}\ntitle: {source}\nsummary: {source}"),
                "instructions",
                &[],
            );
        }
        let registry = registry_with_roots(SkillRoots {
            workspace: Some(workspace.path().to_path_buf()),
            plugin: Some(plugin.path().to_path_buf()),
            user: Some(user.path().to_path_buf()),
            sandbox: Some(sandbox.path().to_path_buf()),
        });
        let result = registry.discover(SkillDiscoverRequest::default()).unwrap();
        let winner = result
            .catalog
            .iter()
            .find(|entry| entry.skill_id == "dup-skill")
            .unwrap();
        assert_eq!(winner.version, "2.0.0");
        assert_eq!(winner.source_kind, SkillSourceKind::Workspace);
        assert_eq!(result.overrides.len(), 3);
        assert!(
            result
                .overrides
                .iter()
                .any(|event| !event.compatible && event.overridden_version.starts_with('1'))
        );
    }

    #[test]
    fn catalog_is_summary_only_and_load_returns_instructions() {
        let workspace = TempDir::new().unwrap();
        write_skill(
            workspace.path(),
            "detail-skill",
            "id: detail-skill\nversion: 1.0.0\ntitle: Detail\nsummary: short",
            "Full instruction body with secrets.",
            &[],
        );
        let registry = registry_with_roots(SkillRoots {
            workspace: Some(workspace.path().to_path_buf()),
            ..Default::default()
        });
        let catalog = registry.discover(SkillDiscoverRequest::default()).unwrap();
        let entry = catalog
            .catalog
            .iter()
            .find(|entry| entry.skill_id == "detail-skill")
            .unwrap();
        assert_eq!(entry.summary, "short");
        assert!(
            !catalog
                .catalog
                .iter()
                .any(|entry| entry.title.contains("secrets"))
        );
        let loaded = registry
            .load(SkillLoadRequest {
                skill_id: "detail-skill".into(),
                generation: None,
            })
            .unwrap();
        assert!(loaded.instructions_text.contains("Full instruction body"));
        assert_eq!(
            loaded.descriptor.instructions.ref_id,
            "mutsuki.agent.skill:detail-skill/instructions"
        );
    }

    #[test]
    fn missing_required_tool_marks_skill_unavailable() {
        let workspace = TempDir::new().unwrap();
        write_skill(
            workspace.path(),
            "needs-tool",
            "id: needs-tool\nversion: 1.0.0\ntitle: Needs Tool\nsummary: x\nrequired_tools:\n  - missing.tool",
            "body",
            &[],
        );
        let tools = ToolRegistry::default();
        tools
            .register(AgentToolDescriptor::new("other.tool", "p", "other"))
            .unwrap();
        let registry = registry_with_roots(SkillRoots {
            workspace: Some(workspace.path().to_path_buf()),
            ..Default::default()
        })
        .with_tool_registry(tools);
        let _ = registry.reload(SkillReloadRequest {});
        let discover = registry
            .discover(SkillDiscoverRequest {
                include_unavailable: true,
            })
            .unwrap();
        let entry = discover
            .catalog
            .iter()
            .find(|entry| entry.skill_id == "needs-tool")
            .unwrap();
        assert!(!entry.available);
        assert!(
            entry
                .unavailable_reasons
                .iter()
                .any(|reason| reason.contains("missing.tool"))
        );
        let err = registry
            .load(SkillLoadRequest {
                skill_id: "needs-tool".into(),
                generation: None,
            })
            .unwrap_err();
        assert_eq!(err.code, "agent.provider_unavailable");
    }

    #[test]
    fn untrusted_source_rejects_side_effect_required_tools() {
        let user = TempDir::new().unwrap();
        write_skill(
            user.path(),
            "unsafe-skill",
            "id: unsafe-skill\nversion: 1.0.0\ntitle: Unsafe\nsummary: x\nrequired_tools:\n  - write.tool",
            "body",
            &[],
        );
        let tools = ToolRegistry::default();
        tools
            .register({
                let mut descriptor = AgentToolDescriptor::new("write.tool", "p", "writes");
                descriptor.side_effect = ToolSideEffect::WorkspaceWrite;
                descriptor
            })
            .unwrap();
        let registry = registry_with_roots(SkillRoots {
            user: Some(user.path().to_path_buf()),
            ..Default::default()
        })
        .with_tool_registry(tools);
        let _ = registry.reload(SkillReloadRequest {});
        let entry = registry
            .discover(SkillDiscoverRequest {
                include_unavailable: true,
            })
            .unwrap()
            .catalog
            .into_iter()
            .find(|entry| entry.skill_id == "unsafe-skill")
            .unwrap();
        assert!(!entry.available);
        assert!(
            entry
                .unavailable_reasons
                .iter()
                .any(|reason| reason.contains("side effects"))
        );
    }

    #[test]
    fn reload_bumps_generation_and_load_can_pin_old_snapshot() {
        let workspace = TempDir::new().unwrap();
        write_skill(
            workspace.path(),
            "gen-skill",
            "id: gen-skill\nversion: 1.0.0\ntitle: Gen\nsummary: v1",
            "v1 instructions",
            &[],
        );
        let registry = registry_with_roots(SkillRoots {
            workspace: Some(workspace.path().to_path_buf()),
            ..Default::default()
        });
        let gen1 = registry
            .discover(SkillDiscoverRequest::default())
            .unwrap()
            .generation;
        write_skill(
            workspace.path(),
            "gen-skill",
            "id: gen-skill\nversion: 1.1.0\ntitle: Gen\nsummary: v2",
            "v2 instructions",
            &[],
        );
        let reload = registry.reload(SkillReloadRequest {}).unwrap();
        assert!(reload.generation > gen1);
        let pinned = registry
            .load(SkillLoadRequest {
                skill_id: "gen-skill".into(),
                generation: Some(gen1),
            })
            .unwrap();
        assert!(pinned.instructions_text.contains("v1 instructions"));
        let current = registry
            .load(SkillLoadRequest {
                skill_id: "gen-skill".into(),
                generation: None,
            })
            .unwrap();
        assert!(current.instructions_text.contains("v2 instructions"));
    }

    #[test]
    fn rejects_path_escape_and_script_resources_without_policy() {
        let workspace = TempDir::new().unwrap();
        write_skill(
            workspace.path(),
            "escape-skill",
            "id: escape-skill\nversion: 1.0.0\ntitle: Escape\nsummary: x\nresources:\n  - ../outside.txt\n  - run.sh",
            "body",
            &[("run.sh", "#!/bin/sh")],
        );
        assert!(
            confine_package_path(&workspace.path().join("escape-skill"), "../outside.txt").is_err()
        );
        let registry = registry_with_roots(SkillRoots {
            workspace: Some(workspace.path().to_path_buf()),
            ..Default::default()
        });
        let entry = registry
            .discover(SkillDiscoverRequest {
                include_unavailable: true,
            })
            .unwrap()
            .catalog
            .into_iter()
            .find(|entry| entry.skill_id == "escape-skill")
            .unwrap();
        assert!(!entry.available);
        assert!(
            entry
                .unavailable_reasons
                .iter()
                .any(|reason| reason.contains("script resource"))
        );
    }

    #[test]
    fn package_hash_is_stable_for_same_content() {
        let workspace = TempDir::new().unwrap();
        write_skill(
            workspace.path(),
            "hash-skill",
            "id: hash-skill\nversion: 1.0.0\ntitle: Hash\nsummary: x\nresources:\n  - note.txt",
            "body",
            &[("note.txt", "hello")],
        );
        let registry = registry_with_roots(SkillRoots {
            workspace: Some(workspace.path().to_path_buf()),
            ..Default::default()
        });
        let first = registry
            .load(SkillLoadRequest {
                skill_id: "hash-skill".into(),
                generation: None,
            })
            .unwrap();
        let second = registry.reload(SkillReloadRequest {}).unwrap();
        let again = registry
            .load(SkillLoadRequest {
                skill_id: "hash-skill".into(),
                generation: Some(second.generation),
            })
            .unwrap();
        assert_eq!(
            first.descriptor.provenance.package_hash,
            again.descriptor.provenance.package_hash
        );
    }
}
