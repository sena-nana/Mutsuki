//! Bot-owned flow catalog, graph validation and atomic published snapshot lifecycle.
//! Generic runtime packages carry plugin extensions; only this crate interprets Bot node metadata.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, RwLock};

use mutsuki_bot_protocol::{
    BOT_FLOW_ERROR_TYPE, BOT_FLOW_NODE_EXTENSION_ID, BOT_FLOW_NODE_EXTENSION_VERSION,
    BotFlowDocument, BotFlowDraft, BotFlowDraftSaveRequest, BotFlowEdgeKind, BotFlowPublishRequest,
    BotFlowPublishedSnapshot, BotFlowStateSnapshot, BotFlowValidationIssue,
    BotFlowValidationResult, BotFlowValidationSeverity, BotNodeCatalogFragment, BotNodeDescriptor,
    BotNodePortDirection, BotNodeRole,
};
use mutsuki_runtime_contracts::{PluginManifest, RuntimeLoadPlan};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BotFlowError {
    #[error("flow revision conflict: expected {expected}, actual {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("flow document is invalid: {0:?}")]
    Invalid(BotFlowValidationResult),
    #[error("flow catalog is invalid: {0}")]
    InvalidCatalog(String),
    #[error("flow repository failed: {0}")]
    Repository(String),
}

pub trait BotFlowRepository: Send + Sync {
    fn snapshot(&self) -> Result<BotFlowStateSnapshot, BotFlowError>;

    fn published_revision(
        &self,
        revision: u64,
    ) -> Result<Option<BotFlowPublishedSnapshot>, BotFlowError>;

    fn save_draft(
        &self,
        request: BotFlowDraftSaveRequest,
        now_ms: i64,
    ) -> Result<BotFlowDraft, BotFlowError>;

    fn discard_draft(&self, expected_revision: u64) -> Result<(), BotFlowError>;

    fn publish(
        &self,
        request: BotFlowPublishRequest,
        now_ms: i64,
    ) -> Result<BotFlowStateSnapshot, BotFlowError>;
}

#[derive(Clone, Debug, Default)]
pub struct BotNodeCatalog {
    nodes: BTreeMap<(String, u32), BotNodeDescriptor>,
}

impl BotNodeCatalog {
    pub fn from_load_plan(plan: &RuntimeLoadPlan) -> Result<Self, BotFlowError> {
        Self::from_manifests(&plan.plugins)
    }

    pub fn from_manifests(manifests: &[PluginManifest]) -> Result<Self, BotFlowError> {
        let bindings = manifests
            .iter()
            .flat_map(|manifest| manifest.provides.handler_bindings.iter())
            .map(|binding| (binding.binding_id.as_str(), binding))
            .collect::<BTreeMap<_, _>>();
        let mut nodes = BTreeMap::new();
        for manifest in manifests {
            for extension in &manifest.provides.extensions {
                if extension.extension_id != BOT_FLOW_NODE_EXTENSION_ID {
                    continue;
                }
                if extension.version != BOT_FLOW_NODE_EXTENSION_VERSION {
                    return Err(BotFlowError::InvalidCatalog(format!(
                        "plugin {} declares unsupported {}@{}",
                        manifest.plugin_id, extension.extension_id, extension.version
                    )));
                }
                let fragment = BotNodeCatalogFragment::from_plugin_extension(extension)
                    .map_err(|error| BotFlowError::InvalidCatalog(error.to_string()))?
                    .ok_or_else(|| {
                        BotFlowError::InvalidCatalog(format!(
                            "plugin {} declares an invalid Bot flow extension",
                            manifest.plugin_id
                        ))
                    })?;
                for node in fragment.nodes {
                    validate_node_descriptor(&node, &bindings)?;
                    let key = (node.node_type_id.clone(), node.version);
                    if nodes.insert(key.clone(), node).is_some() {
                        return Err(BotFlowError::InvalidCatalog(format!(
                            "duplicate node type {}@{}",
                            key.0, key.1
                        )));
                    }
                }
            }
        }
        Ok(Self { nodes })
    }

    pub fn descriptor(&self, node_type_id: &str, version: u32) -> Option<&BotNodeDescriptor> {
        self.nodes.get(&(node_type_id.to_owned(), version))
    }

    pub fn descriptors(&self) -> Vec<BotNodeDescriptor> {
        self.nodes.values().cloned().collect()
    }
}

fn validate_node_descriptor(
    node: &BotNodeDescriptor,
    bindings: &BTreeMap<&str, &mutsuki_runtime_contracts::HandlerBinding>,
) -> Result<(), BotFlowError> {
    if node.node_type_id.trim().is_empty() || node.version == 0 || !node.config_schema.is_object() {
        return Err(BotFlowError::InvalidCatalog(format!(
            "invalid node descriptor {}@{}",
            node.node_type_id, node.version
        )));
    }
    let mut ports = BTreeSet::new();
    for port in &node.ports {
        if port.port_id.trim().is_empty()
            || port.event_type.type_id.trim().is_empty()
            || port.event_type.version == 0
            || !ports.insert((port.direction, port.port_id.as_str()))
        {
            return Err(BotFlowError::InvalidCatalog(format!(
                "invalid or duplicate port on {}@{}",
                node.node_type_id, node.version
            )));
        }
    }
    match (&node.role, &node.binding) {
        (BotNodeRole::Source, None) => {}
        (BotNodeRole::Source, Some(_)) | (_, None) => {
            return Err(BotFlowError::InvalidCatalog(format!(
                "node {}@{} has invalid binding for its role",
                node.node_type_id, node.version
            )));
        }
        (_, Some(binding)) => {
            let Some(known) = bindings.get(binding.binding_id.as_str()) else {
                return Err(BotFlowError::InvalidCatalog(format!(
                    "node {}@{} references missing binding {}",
                    node.node_type_id, node.version, binding.binding_id
                )));
            };
            if known.target_protocol_id != binding.protocol_id
                || binding
                    .runner_hint
                    .as_ref()
                    .is_some_and(|hint| known.target_runner_hint.as_ref() != Some(hint))
            {
                return Err(BotFlowError::InvalidCatalog(format!(
                    "node {}@{} binding {} is incompatible",
                    node.node_type_id, node.version, binding.binding_id
                )));
            }
        }
    }
    Ok(())
}

pub fn validate_flows(
    flows: &[BotFlowDocument],
    catalog: &BotNodeCatalog,
) -> BotFlowValidationResult {
    let mut issues = Vec::new();
    let mut flow_ids = BTreeSet::new();
    for flow in flows {
        if flow.flow_id.trim().is_empty() || !flow_ids.insert(flow.flow_id.as_str()) {
            push_issue(
                &mut issues,
                "flow.duplicate_id",
                "流程 ID 为空或重复",
                Some(&flow.flow_id),
                None,
                None,
                None,
            );
        }
        validate_flow(flow, catalog, &mut issues);
    }
    BotFlowValidationResult {
        valid: issues
            .iter()
            .all(|issue| issue.severity != BotFlowValidationSeverity::Error),
        issues,
    }
}

fn validate_flow(
    flow: &BotFlowDocument,
    catalog: &BotNodeCatalog,
    issues: &mut Vec<BotFlowValidationIssue>,
) {
    let mut nodes = BTreeMap::new();
    let mut has_source = false;
    for node in &flow.nodes {
        if node.node_id.trim().is_empty() || nodes.insert(node.node_id.as_str(), node).is_some() {
            push_issue(
                issues,
                "flow.node.duplicate_id",
                "节点 ID 为空或重复",
                Some(&flow.flow_id),
                Some(&node.node_id),
                None,
                None,
            );
            continue;
        }
        let Some(descriptor) = catalog.descriptor(&node.node_type_id, node.node_type_version)
        else {
            push_issue(
                issues,
                "flow.node.unavailable",
                "节点类型在当前插件目录中不可用",
                Some(&flow.flow_id),
                Some(&node.node_id),
                None,
                None,
            );
            continue;
        };
        if !node.config.is_object() {
            push_issue(
                issues,
                "flow.node.config_type",
                "节点配置必须是对象",
                Some(&flow.flow_id),
                Some(&node.node_id),
                None,
                Some("config"),
            );
        } else {
            let mut schema_errors = Vec::new();
            validate_schema_value(
                &descriptor.config_schema,
                &node.config,
                "config",
                &mut schema_errors,
            );
            for (path, message) in schema_errors {
                push_issue(
                    issues,
                    "flow.node.config_invalid",
                    &message,
                    Some(&flow.flow_id),
                    Some(&node.node_id),
                    None,
                    Some(&path),
                );
            }
        }
        if descriptor.role == BotNodeRole::Source {
            has_source = true;
            if node
                .source
                .as_ref()
                .is_none_or(|source| source.protocol_id.trim().is_empty())
            {
                push_issue(
                    issues,
                    "flow.source.selector_required",
                    "输入节点缺少事件来源",
                    Some(&flow.flow_id),
                    Some(&node.node_id),
                    None,
                    Some("source"),
                );
            }
        } else if node.source.is_some() {
            push_issue(
                issues,
                "flow.source.selector_forbidden",
                "仅输入节点可以声明事件来源",
                Some(&flow.flow_id),
                Some(&node.node_id),
                None,
                Some("source"),
            );
        }
    }
    if !has_source {
        push_issue(
            issues,
            "flow.source.missing",
            "流程至少需要一个输入节点",
            Some(&flow.flow_id),
            None,
            None,
            None,
        );
    }

    let mut edge_ids = BTreeSet::new();
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut indegree = nodes
        .keys()
        .map(|id| (*id, 0_usize))
        .collect::<BTreeMap<_, _>>();
    for edge in &flow.edges {
        if edge.edge_id.trim().is_empty() || !edge_ids.insert(edge.edge_id.as_str()) {
            push_issue(
                issues,
                "flow.edge.duplicate_id",
                "连线 ID 为空或重复",
                Some(&flow.flow_id),
                None,
                Some(&edge.edge_id),
                None,
            );
        }
        let (Some(from), Some(to)) = (
            nodes.get(edge.from_node_id.as_str()),
            nodes.get(edge.to_node_id.as_str()),
        ) else {
            push_issue(
                issues,
                "flow.edge.dangling",
                "连线引用了不存在的节点",
                Some(&flow.flow_id),
                None,
                Some(&edge.edge_id),
                None,
            );
            continue;
        };
        let Some(from_descriptor) = catalog.descriptor(&from.node_type_id, from.node_type_version)
        else {
            continue;
        };
        let Some(to_descriptor) = catalog.descriptor(&to.node_type_id, to.node_type_version) else {
            continue;
        };
        let to_port = to_descriptor.ports.iter().find(|port| {
            port.direction == BotNodePortDirection::Input && port.port_id == edge.to_port_id
        });
        let error_type = mutsuki_bot_protocol::BotFlowTypeRef {
            type_id: BOT_FLOW_ERROR_TYPE.into(),
            version: 1,
        };
        let from_type = match edge.kind {
            BotFlowEdgeKind::Event => from_descriptor
                .ports
                .iter()
                .find(|port| {
                    port.direction == BotNodePortDirection::Output
                        && port.port_id == edge.from_port_id
                })
                .map(|port| &port.event_type),
            BotFlowEdgeKind::Error => Some(&error_type),
        };
        match (from_type, to_port) {
            (Some(from_type), Some(to_port)) if from_type == &to_port.event_type => {}
            (None, _) | (_, None) => push_issue(
                issues,
                "flow.edge.port_missing",
                "连线引用了不存在的端口",
                Some(&flow.flow_id),
                None,
                Some(&edge.edge_id),
                None,
            ),
            _ => push_issue(
                issues,
                "flow.edge.type_mismatch",
                "连线两端的事件类型不兼容",
                Some(&flow.flow_id),
                None,
                Some(&edge.edge_id),
                None,
            ),
        }
        adjacency
            .entry(edge.from_node_id.as_str())
            .or_default()
            .push(edge.to_node_id.as_str());
        *indegree.entry(edge.to_node_id.as_str()).or_default() += 1;
    }
    let mut ready = indegree
        .iter()
        .filter_map(|(node_id, degree)| (*degree == 0).then_some(*node_id))
        .collect::<VecDeque<_>>();
    let mut visited = 0;
    while let Some(node_id) = ready.pop_front() {
        visited += 1;
        for target in adjacency.get(node_id).into_iter().flatten() {
            let degree = indegree
                .get_mut(target)
                .expect("validated target has indegree");
            *degree -= 1;
            if *degree == 0 {
                ready.push_back(target);
            }
        }
    }
    if visited != nodes.len() {
        push_issue(
            issues,
            "flow.cycle",
            "第一版流程不允许循环",
            Some(&flow.flow_id),
            None,
            None,
            None,
        );
    }
}

fn validate_schema_value(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
    errors: &mut Vec<(String, String)>,
) {
    if let Some(allowed) = schema.get("enum").and_then(serde_json::Value::as_array)
        && !allowed.contains(value)
    {
        errors.push((path.into(), "配置值不在允许范围内".into()));
        return;
    }
    let Some(kind) = schema.get("type").and_then(serde_json::Value::as_str) else {
        return;
    };
    let valid_type = match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    if !valid_type {
        errors.push((path.into(), format!("配置值必须是 {kind}")));
        return;
    }
    match kind {
        "object" => {
            let object = value.as_object().expect("validated object");
            let properties = schema
                .get("properties")
                .and_then(serde_json::Value::as_object);
            for required in schema
                .get("required")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
            {
                if !object.contains_key(required) {
                    errors.push((format!("{path}.{required}"), "缺少必填配置".into()));
                }
            }
            if schema
                .get("additionalProperties")
                .and_then(serde_json::Value::as_bool)
                == Some(false)
            {
                for key in object.keys() {
                    if properties.is_none_or(|known| !known.contains_key(key)) {
                        errors.push((format!("{path}.{key}"), "配置字段未在 schema 中声明".into()));
                    }
                }
            }
            if let Some(properties) = properties {
                for (key, child_schema) in properties {
                    if let Some(child) = object.get(key) {
                        validate_schema_value(
                            child_schema,
                            child,
                            &format!("{path}.{key}"),
                            errors,
                        );
                    }
                }
            }
        }
        "array" => {
            let array = value.as_array().expect("validated array");
            if let Some(minimum) = schema.get("minItems").and_then(serde_json::Value::as_u64)
                && u64::try_from(array.len()).is_ok_and(|length| length < minimum)
            {
                errors.push((path.into(), format!("配置列表至少需要 {minimum} 项")));
            }
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in array.iter().enumerate() {
                    validate_schema_value(item_schema, item, &format!("{path}[{index}]"), errors);
                }
            }
        }
        "string" => {
            if let Some(minimum) = schema.get("minLength").and_then(serde_json::Value::as_u64)
                && u64::try_from(value.as_str().expect("validated string").chars().count())
                    .is_ok_and(|length| length < minimum)
            {
                errors.push((path.into(), format!("配置文本至少需要 {minimum} 个字符")));
            }
        }
        "integer" | "number" => {
            let number = value.as_f64().expect("validated number");
            if schema
                .get("minimum")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|minimum| number < minimum)
            {
                errors.push((path.into(), "配置值低于最小值".into()));
            }
            if schema
                .get("maximum")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|maximum| number > maximum)
            {
                errors.push((path.into(), "配置值高于最大值".into()));
            }
        }
        _ => {}
    }
}

fn push_issue(
    issues: &mut Vec<BotFlowValidationIssue>,
    code: &str,
    message: &str,
    flow_id: Option<&str>,
    node_id: Option<&str>,
    edge_id: Option<&str>,
    path: Option<&str>,
) {
    issues.push(BotFlowValidationIssue {
        code: code.into(),
        message: message.into(),
        severity: BotFlowValidationSeverity::Error,
        flow_id: flow_id.map(str::to_owned),
        node_id: node_id.map(str::to_owned),
        edge_id: edge_id.map(str::to_owned),
        path: path.map(str::to_owned),
    });
}

pub struct BotFlowRegistry {
    repository: Arc<dyn BotFlowRepository>,
    catalog: RwLock<BotNodeCatalog>,
    active: RwLock<Arc<BotFlowPublishedSnapshot>>,
}

impl BotFlowRegistry {
    pub fn restore(repository: Arc<dyn BotFlowRepository>) -> Result<Self, BotFlowError> {
        let snapshot = repository.snapshot()?;
        Ok(Self {
            repository,
            catalog: RwLock::new(BotNodeCatalog::default()),
            active: RwLock::new(Arc::new(snapshot.published)),
        })
    }

    pub fn open(
        repository: Arc<dyn BotFlowRepository>,
        catalog: BotNodeCatalog,
    ) -> Result<Self, BotFlowError> {
        let snapshot = repository.snapshot()?;
        let validation = validate_flows(&snapshot.published.flows, &catalog);
        if !validation.valid {
            return Err(BotFlowError::Invalid(validation));
        }
        Ok(Self {
            repository,
            catalog: RwLock::new(catalog),
            active: RwLock::new(Arc::new(snapshot.published)),
        })
    }

    pub fn catalog(&self) -> Vec<BotNodeDescriptor> {
        self.catalog
            .read()
            .expect("Bot flow catalog lock poisoned")
            .descriptors()
    }

    pub fn descriptor(&self, node_type_id: &str, version: u32) -> Option<BotNodeDescriptor> {
        self.catalog
            .read()
            .expect("Bot flow catalog lock poisoned")
            .descriptor(node_type_id, version)
            .cloned()
    }

    pub fn snapshot(&self) -> Result<BotFlowStateSnapshot, BotFlowError> {
        self.repository.snapshot()
    }

    pub fn active(&self) -> Arc<BotFlowPublishedSnapshot> {
        self.active
            .read()
            .expect("Bot flow active lock poisoned")
            .clone()
    }

    pub fn published_revision(
        &self,
        revision: u64,
    ) -> Result<Arc<BotFlowPublishedSnapshot>, BotFlowError> {
        let active = self.active();
        if active.revision == revision {
            return Ok(active);
        }
        self.repository
            .published_revision(revision)?
            .map(Arc::new)
            .ok_or_else(|| BotFlowError::Repository(format!("missing graph revision {revision}")))
    }

    pub fn validate(&self, flows: &[BotFlowDocument]) -> BotFlowValidationResult {
        validate_flows(
            flows,
            &self.catalog.read().expect("Bot flow catalog lock poisoned"),
        )
    }

    pub fn save_draft(
        &self,
        request: BotFlowDraftSaveRequest,
        now_ms: i64,
    ) -> Result<BotFlowDraft, BotFlowError> {
        self.repository.save_draft(request, now_ms)
    }

    pub fn discard_draft(&self, expected_revision: u64) -> Result<(), BotFlowError> {
        self.repository.discard_draft(expected_revision)
    }

    pub fn publish(
        &self,
        request: BotFlowPublishRequest,
        now_ms: i64,
    ) -> Result<Arc<BotFlowPublishedSnapshot>, BotFlowError> {
        let snapshot = self.repository.snapshot()?;
        let Some(draft) = snapshot.draft.as_ref() else {
            return Err(BotFlowError::RevisionConflict {
                expected: request.expected_draft_revision,
                actual: 0,
            });
        };
        let validation = self.validate(&draft.flows);
        if !validation.valid {
            return Err(BotFlowError::Invalid(validation));
        }
        let persisted = self.repository.publish(request, now_ms)?;
        let published = Arc::new(persisted.published);
        *self.active.write().expect("Bot flow active lock poisoned") = published.clone();
        Ok(published)
    }

    pub fn validate_load_plan(&self, plan: &RuntimeLoadPlan) -> Result<(), BotFlowError> {
        self.validated_catalog(plan).map(drop)
    }

    pub fn activate_load_plan(&self, plan: &RuntimeLoadPlan) -> Result<(), BotFlowError> {
        let candidate = self.validated_catalog(plan)?;
        *self
            .catalog
            .write()
            .expect("Bot flow catalog lock poisoned") = candidate;
        Ok(())
    }

    fn validated_catalog(&self, plan: &RuntimeLoadPlan) -> Result<BotNodeCatalog, BotFlowError> {
        let candidate = BotNodeCatalog::from_load_plan(plan)?;
        let validation = validate_flows(&self.active().flows, &candidate);
        validation
            .valid
            .then_some(candidate)
            .ok_or(BotFlowError::Invalid(validation))
    }
}

#[derive(Default)]
pub struct InMemoryBotFlowRepository {
    state: RwLock<InMemoryState>,
}

#[derive(Clone)]
struct InMemoryState {
    draft: Option<BotFlowDraft>,
    published: BotFlowPublishedSnapshot,
    versions: BTreeMap<u64, BotFlowPublishedSnapshot>,
}

impl Default for InMemoryState {
    fn default() -> Self {
        let published = BotFlowPublishedSnapshot {
            revision: 0,
            flows: Vec::new(),
            published_at_ms: 0,
        };
        Self {
            draft: None,
            published,
            versions: BTreeMap::new(),
        }
    }
}

impl BotFlowRepository for InMemoryBotFlowRepository {
    fn snapshot(&self) -> Result<BotFlowStateSnapshot, BotFlowError> {
        let state = self.state.read().expect("memory flow lock poisoned");
        Ok(BotFlowStateSnapshot {
            draft: state.draft.clone(),
            published: state.published.clone(),
        })
    }

    fn published_revision(
        &self,
        revision: u64,
    ) -> Result<Option<BotFlowPublishedSnapshot>, BotFlowError> {
        let state = self.state.read().expect("memory flow lock poisoned");
        if state.published.revision == revision {
            Ok(Some(state.published.clone()))
        } else {
            Ok(state.versions.get(&revision).cloned())
        }
    }

    fn save_draft(
        &self,
        request: BotFlowDraftSaveRequest,
        now_ms: i64,
    ) -> Result<BotFlowDraft, BotFlowError> {
        let mut state = self.state.write().expect("memory flow lock poisoned");
        let actual = state.draft.as_ref().map_or(0, |draft| draft.revision);
        if request.expected_draft_revision.unwrap_or(0) != actual
            || request.base_published_revision != state.published.revision
        {
            return Err(BotFlowError::RevisionConflict {
                expected: request.expected_draft_revision.unwrap_or(0),
                actual,
            });
        }
        let draft = BotFlowDraft {
            revision: actual + 1,
            base_published_revision: request.base_published_revision,
            flows: request.flows,
            updated_at_ms: now_ms,
        };
        state.draft = Some(draft.clone());
        Ok(draft)
    }

    fn discard_draft(&self, expected_revision: u64) -> Result<(), BotFlowError> {
        let mut state = self.state.write().expect("memory flow lock poisoned");
        let actual = state.draft.as_ref().map_or(0, |draft| draft.revision);
        if actual != expected_revision {
            return Err(BotFlowError::RevisionConflict {
                expected: expected_revision,
                actual,
            });
        }
        state.draft = None;
        Ok(())
    }

    fn publish(
        &self,
        request: BotFlowPublishRequest,
        now_ms: i64,
    ) -> Result<BotFlowStateSnapshot, BotFlowError> {
        let mut state = self.state.write().expect("memory flow lock poisoned");
        if state.published.revision != request.expected_published_revision {
            return Err(BotFlowError::RevisionConflict {
                expected: request.expected_published_revision,
                actual: state.published.revision,
            });
        }
        let Some(draft) = state.draft.as_ref() else {
            return Err(BotFlowError::RevisionConflict {
                expected: request.expected_draft_revision,
                actual: 0,
            });
        };
        if draft.revision != request.expected_draft_revision
            || draft.base_published_revision != state.published.revision
        {
            return Err(BotFlowError::RevisionConflict {
                expected: request.expected_draft_revision,
                actual: draft.revision,
            });
        }
        let published = BotFlowPublishedSnapshot {
            revision: state.published.revision + 1,
            flows: draft.flows.clone(),
            published_at_ms: now_ms,
        };
        let previous = state.published.clone();
        state.versions.insert(previous.revision, previous);
        state.published = published;
        state.draft = None;
        Ok(BotFlowStateSnapshot {
            draft: None,
            published: state.published.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{
        BOT_FLOW_BOT_EVENT_TYPE, BotFlowEdge, BotFlowNode, BotFlowNodePosition,
        BotFlowSourceSelector, BotFlowTypeRef, BotNodeBinding, BotNodeCatalogFragment,
        BotNodePortDescriptor,
    };
    use mutsuki_runtime_sdk::{PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder};
    use serde_json::json;

    const PROTOCOL: &str = "test.bot.node/process@1";
    const RUNNER: &str = "test.bot.node.runner";

    #[test]
    fn validation_rejects_schema_errors_cycles_and_type_mismatches() {
        let catalog = catalog();
        let mut flow = valid_flow();
        flow.nodes[1].config = json!({});
        flow.edges.push(BotFlowEdge {
            edge_id: "cycle".into(),
            from_node_id: "processor".into(),
            from_port_id: "event".into(),
            to_node_id: "source".into(),
            to_port_id: "missing".into(),
            kind: BotFlowEdgeKind::Event,
        });

        let result = validate_flows(&[flow], &catalog);

        assert!(!result.valid);
        let codes = result
            .issues
            .iter()
            .map(|issue| issue.code.as_str())
            .collect::<BTreeSet<_>>();
        assert!(codes.contains("flow.node.config_invalid"));
        assert!(codes.contains("flow.edge.port_missing"));
        assert!(codes.contains("flow.cycle"));
    }

    #[test]
    fn publish_uses_cas_and_keeps_old_graph_revisions_immutable() {
        let repository = Arc::new(InMemoryBotFlowRepository::default());
        let registry = BotFlowRegistry::open(repository, catalog()).unwrap();
        let first = registry
            .save_draft(
                BotFlowDraftSaveRequest {
                    expected_draft_revision: None,
                    base_published_revision: 0,
                    flows: vec![valid_flow()],
                },
                10,
            )
            .unwrap();
        let published = registry
            .publish(
                BotFlowPublishRequest {
                    expected_draft_revision: first.revision,
                    expected_published_revision: 0,
                },
                11,
            )
            .unwrap();
        assert_eq!(published.revision, 1);

        let mut changed = valid_flow();
        changed.name = "changed".into();
        let second = registry
            .save_draft(
                BotFlowDraftSaveRequest {
                    expected_draft_revision: None,
                    base_published_revision: 1,
                    flows: vec![changed],
                },
                12,
            )
            .unwrap();
        let conflict = registry.publish(
            BotFlowPublishRequest {
                expected_draft_revision: second.revision,
                expected_published_revision: 0,
            },
            13,
        );
        assert!(matches!(
            conflict,
            Err(BotFlowError::RevisionConflict { .. })
        ));
        registry
            .publish(
                BotFlowPublishRequest {
                    expected_draft_revision: second.revision,
                    expected_published_revision: 1,
                },
                14,
            )
            .unwrap();
        assert_eq!(
            registry.published_revision(1).unwrap().flows[0].name,
            "valid"
        );
        assert_eq!(registry.active().flows[0].name, "changed");
    }

    fn catalog() -> BotNodeCatalog {
        let event_type = BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1);
        let fragment = BotNodeCatalogFragment {
            nodes: vec![
                BotNodeDescriptor {
                    node_type_id: "test.source".into(),
                    version: 1,
                    title: "Source".into(),
                    category: "Test".into(),
                    role: BotNodeRole::Source,
                    binding: None,
                    ports: vec![BotNodePortDescriptor {
                        port_id: "event".into(),
                        title: "Event".into(),
                        direction: BotNodePortDirection::Output,
                        event_type: event_type.clone(),
                        required: false,
                    }],
                    config_schema: json!({"type": "object", "additionalProperties": false}),
                },
                BotNodeDescriptor {
                    node_type_id: "test.processor".into(),
                    version: 1,
                    title: "Processor".into(),
                    category: "Test".into(),
                    role: BotNodeRole::Processor,
                    binding: Some(BotNodeBinding {
                        binding_id: format!("binding:{PROTOCOL}"),
                        protocol_id: PROTOCOL.into(),
                        runner_hint: Some(RUNNER.into()),
                    }),
                    ports: vec![
                        BotNodePortDescriptor {
                            port_id: "event".into(),
                            title: "Input".into(),
                            direction: BotNodePortDirection::Input,
                            event_type: event_type.clone(),
                            required: true,
                        },
                        BotNodePortDescriptor {
                            port_id: "event".into(),
                            title: "Output".into(),
                            direction: BotNodePortDirection::Output,
                            event_type,
                            required: false,
                        },
                    ],
                    config_schema: json!({
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["mode"],
                        "properties": {"mode": {"type": "string", "enum": ["pass"]}}
                    }),
                },
            ],
        };
        let manifest = PluginBuilder::new("test.bot.nodes")
            .runner_descriptor(
                RunnerDescriptorBuilder::new(RUNNER, "test.bot.nodes")
                    .accepted_protocol(PROTOCOL)
                    .build(),
            )
            .protocol_handler(
                ProtocolDescriptorBuilder::new(PROTOCOL).build(),
                RUNNER,
                "test-node",
            )
            .extension(fragment.into_plugin_extension().unwrap())
            .build()
            .manifest;
        BotNodeCatalog::from_manifests(&[manifest]).unwrap()
    }

    fn valid_flow() -> BotFlowDocument {
        BotFlowDocument {
            flow_id: "flow".into(),
            name: "valid".into(),
            enabled: true,
            nodes: vec![
                BotFlowNode {
                    node_id: "source".into(),
                    node_type_id: "test.source".into(),
                    node_type_version: 1,
                    config: json!({}),
                    source: Some(BotFlowSourceSelector {
                        protocol_id: "mutsuki.bot.event/ingest@1".into(),
                        event_type: None,
                    }),
                    position: BotFlowNodePosition::default(),
                },
                BotFlowNode {
                    node_id: "processor".into(),
                    node_type_id: "test.processor".into(),
                    node_type_version: 1,
                    config: json!({"mode": "pass"}),
                    source: None,
                    position: BotFlowNodePosition::default(),
                },
            ],
            edges: vec![BotFlowEdge {
                edge_id: "edge".into(),
                from_node_id: "source".into(),
                from_port_id: "event".into(),
                to_node_id: "processor".into(),
                to_port_id: "event".into(),
                kind: BotFlowEdgeKind::Event,
            }],
        }
    }
}
