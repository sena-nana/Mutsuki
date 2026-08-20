//! Bot-owned flow catalog, graph validation and atomic active snapshot lifecycle.
//! Generic runtime packages carry plugin extensions; only this crate interprets Bot node metadata.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use mutsuki_bot_protocol::{
    BOT_FLOW_ERROR_TYPE, BOT_FLOW_NODE_EXTENSION_ID, BOT_FLOW_NODE_EXTENSION_VERSION,
    BotFlowDocument, BotFlowEdgeKind, BotFlowSnapshot, BotFlowValidationIssue,
    BotFlowValidationResult, BotFlowValidationSeverity, BotNodeCatalogFragment, BotNodeDescriptor,
    BotNodePortDirection, BotNodeRole,
};
use mutsuki_config_service::{
    ConfigActivation, ConfigConstraints, ConfigContext, ConfigDescriptor, ConfigError, ConfigKey,
    ConfigMutability, ConfigNode, ConfigPath, ConfigPresentation, ConfigProvider, ConfigProviderId,
    ConfigRevision, ConfigScope, ConfigSnapshot, ConfigValue, ConfigValueType, LocalizedText,
    PreparedConfigActivation, RestartPolicy, ValidationCode, ValidationIssue, ValidationResult,
    ValidationSeverity,
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

pub fn validate_flow(flow: &BotFlowDocument, catalog: &BotNodeCatalog) -> BotFlowValidationResult {
    let mut issues = Vec::new();
    if flow.flow_id.trim().is_empty() {
        push_issue(
            &mut issues,
            "flow.missing_id",
            "流程 ID 为空",
            Some(&flow.flow_id),
            None,
            None,
            None,
        );
    }
    validate_graph(flow, catalog, &mut issues);
    BotFlowValidationResult {
        valid: issues
            .iter()
            .all(|issue| issue.severity != BotFlowValidationSeverity::Error),
        issues,
    }
}

fn validate_graph(
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
    if !flow.nodes.is_empty() && !has_source {
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
            (Some(from_type), Some(to_port)) if from_type.assigns_to(&to_port.event_type) => {}
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

pub const BOT_FLOW_CONFIG_PROVIDER_ID: &str = "mutsuki.bot.flow";

pub struct BotFlowRegistry {
    catalog: RwLock<BotNodeCatalog>,
    active: RwLock<Arc<BotFlowSnapshot>>,
}

impl BotFlowRegistry {
    #[must_use]
    pub fn new(catalog: BotNodeCatalog) -> Self {
        Self {
            catalog: RwLock::new(catalog),
            active: RwLock::new(Arc::new(BotFlowSnapshot {
                revision: 0,
                flow: BotFlowDocument::default(),
            })),
        }
    }

    /// Constructs an already-active immutable snapshot for owner assembly and tests.
    /// Persistence remains the responsibility of `ConfigService` in production.
    pub fn with_snapshot(
        catalog: BotNodeCatalog,
        snapshot: BotFlowSnapshot,
    ) -> Result<Self, BotFlowError> {
        let validation = validate_flow(&snapshot.flow, &catalog);
        if !validation.valid {
            return Err(BotFlowError::Invalid(validation));
        }
        Ok(Self {
            catalog: RwLock::new(catalog),
            active: RwLock::new(Arc::new(snapshot)),
        })
    }

    #[must_use]
    pub fn catalog(&self) -> Vec<BotNodeDescriptor> {
        self.catalog
            .read()
            .expect("Bot flow catalog lock poisoned")
            .descriptors()
    }

    #[must_use]
    pub fn descriptor(&self, node_type_id: &str, version: u32) -> Option<BotNodeDescriptor> {
        self.catalog
            .read()
            .expect("Bot flow catalog lock poisoned")
            .descriptor(node_type_id, version)
            .cloned()
    }

    #[must_use]
    pub fn active(&self) -> Arc<BotFlowSnapshot> {
        self.active
            .read()
            .expect("Bot flow active lock poisoned")
            .clone()
    }

    #[must_use]
    pub fn validate(&self, flow: &BotFlowDocument) -> BotFlowValidationResult {
        validate_flow(
            flow,
            &self.catalog.read().expect("Bot flow catalog lock poisoned"),
        )
    }

    pub fn validate_load_plan(&self, plan: &RuntimeLoadPlan) -> Result<(), BotFlowError> {
        BotNodeCatalog::from_load_plan(plan).map(drop)
    }

    pub fn activate_load_plan(&self, plan: &RuntimeLoadPlan) -> Result<(), BotFlowError> {
        let candidate = BotNodeCatalog::from_load_plan(plan)?;
        let validation = validate_flow(&self.active().flow, &candidate);
        *self
            .catalog
            .write()
            .expect("Bot flow catalog lock poisoned") = candidate;
        if !validation.valid {
            *self.active.write().expect("Bot flow active lock poisoned") =
                Arc::new(BotFlowSnapshot {
                    revision: 0,
                    flow: BotFlowDocument::default(),
                });
        }
        Ok(())
    }
}

struct FlowActivation {
    registry: Arc<BotFlowRegistry>,
    previous: Arc<BotFlowSnapshot>,
    candidate: Arc<BotFlowSnapshot>,
    activated: bool,
    finished: bool,
}

impl ConfigActivation for FlowActivation {
    fn activate(&mut self) -> Result<(), ConfigError> {
        *self
            .registry
            .active
            .write()
            .expect("Bot flow active lock poisoned") = self.candidate.clone();
        self.activated = true;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), ConfigError> {
        self.finished = true;
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), ConfigError> {
        if self.activated {
            *self
                .registry
                .active
                .write()
                .expect("Bot flow active lock poisoned") = self.previous.clone();
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for FlowActivation {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.rollback();
        }
    }
}

pub struct BotFlowConfigProvider {
    registry: Arc<BotFlowRegistry>,
}

impl BotFlowConfigProvider {
    #[must_use]
    pub fn new(registry: Arc<BotFlowRegistry>) -> Self {
        Self { registry }
    }

    pub fn decode(value: &ConfigValue) -> Result<BotFlowDocument, ConfigError> {
        let json = value.to_json();
        if let Some(flow) = json.get("flow").filter(|value| !value.is_null()) {
            return serde_json::from_value(flow.clone()).map_err(invalid_flow);
        }
        if let Some(flows) = json.get("flows") {
            let flows: Vec<BotFlowDocument> =
                serde_json::from_value(flows.clone()).map_err(invalid_flow)?;
            return match flows.len() {
                0 => Ok(BotFlowDocument::default()),
                1 => Ok(flows.into_iter().next().expect("one legacy flow")),
                count => Err(ConfigError::ApplyRejected {
                    reason: format!("Bot Flow allows exactly one flow document, found {count}"),
                }),
            };
        }
        Ok(BotFlowDocument::default())
    }

    pub fn encode(flow: &BotFlowDocument) -> Result<ConfigValue, ConfigError> {
        serde_json::to_value(flow)
            .map(|flow| ConfigValue::from_json(&serde_json::json!({ "flow": flow })))
            .map_err(|error| ConfigError::ApplyRejected {
                reason: format!("failed to encode Bot Flow document: {error}"),
            })
    }

    fn validation(&self, flow: &BotFlowDocument) -> ValidationResult {
        let result = self.registry.validate(flow);
        ValidationResult::from_issues(
            result
                .issues
                .into_iter()
                .map(|issue| ValidationIssue {
                    path: ConfigPath(
                        issue
                            .path
                            .into_iter()
                            .chain(issue.flow_id)
                            .chain(issue.node_id)
                            .chain(issue.edge_id)
                            .collect(),
                    ),
                    code: ValidationCode::BusinessRule,
                    severity: match issue.severity {
                        BotFlowValidationSeverity::Error => ValidationSeverity::Error,
                        BotFlowValidationSeverity::Warning => ValidationSeverity::Warning,
                    },
                    message: LocalizedText::new(issue.message),
                })
                .collect(),
        )
    }
}

fn invalid_flow(error: serde_json::Error) -> ConfigError {
    ConfigError::ApplyRejected {
        reason: format!("invalid Bot Flow document: {error}"),
    }
}

#[async_trait]
impl ConfigProvider for BotFlowConfigProvider {
    fn descriptor(&self) -> ConfigDescriptor {
        ConfigDescriptor {
            provider_id: ConfigProviderId::new(BOT_FLOW_CONFIG_PROVIDER_ID),
            schema_version: 1,
            value_version: 2,
            title: LocalizedText::new("Bot Flow"),
            description: None,
            scopes: vec![ConfigScope::global()],
            root: ConfigNode {
                key: ConfigKey::new("root"),
                value_type: ConfigValueType::Object,
                title: LocalizedText::new("Bot Flow"),
                description: None,
                default_value: None,
                constraints: ConfigConstraints::default(),
                presentation: ConfigPresentation::default(),
                visibility: None,
                enabled_if: None,
                mutability: ConfigMutability::ReadWrite,
                restart_policy: RestartPolicy::None,
                children: vec![ConfigNode {
                    key: ConfigKey::new("flow"),
                    value_type: ConfigValueType::Object,
                    title: LocalizedText::new("Flow"),
                    description: None,
                    default_value: None,
                    constraints: ConfigConstraints::default(),
                    presentation: ConfigPresentation::default(),
                    visibility: None,
                    enabled_if: None,
                    mutability: ConfigMutability::ReadWrite,
                    restart_policy: RestartPolicy::None,
                    children: Vec::new(),
                }],
            },
            groups: Vec::new(),
        }
    }

    fn default_value(&self, _context: &ConfigContext) -> Result<ConfigValue, ConfigError> {
        Self::encode(&BotFlowDocument::default())
    }

    async fn validate(
        &self,
        candidate: ConfigValue,
        _context: ConfigContext,
    ) -> Result<ValidationResult, ConfigError> {
        Ok(self.validation(&Self::decode(&candidate)?))
    }

    async fn prepare_activation(
        &self,
        candidate: ConfigValue,
        _current: ConfigSnapshot,
        next_revision: ConfigRevision,
        _context: ConfigContext,
    ) -> Result<PreparedConfigActivation, ConfigError> {
        let flow = Self::decode(&candidate)?;
        let validation = self.validation(&flow);
        if !validation.ok {
            return Err(ConfigError::ValidationFailed { result: validation });
        }
        let persisted = Self::encode(&flow)?;
        Ok(PreparedConfigActivation::new(
            persisted.clone(),
            Box::new(FlowActivation {
                registry: self.registry.clone(),
                previous: self.registry.active(),
                candidate: Arc::new(BotFlowSnapshot {
                    revision: next_revision.0,
                    flow,
                }),
                activated: false,
                finished: false,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{
        BotFlowEdge, BotFlowNode, BotFlowNodePosition, BotFlowSourceSelector, BotFlowTypeRef,
        BotNodePortDescriptor,
    };
    use mutsuki_config_service::{
        ConfigApplyRequest, ConfigProviderRegistry, ConfigService, InMemoryConfigRepository,
        capability,
    };

    fn catalog() -> BotNodeCatalog {
        let event = BotFlowTypeRef::new("test.event", 1);
        BotNodeCatalog {
            nodes: BTreeMap::from([(
                ("test.source".into(), 1),
                BotNodeDescriptor {
                    node_type_id: "test.source".into(),
                    version: 1,
                    title: "Source".into(),
                    category: "test".into(),
                    role: BotNodeRole::Source,
                    binding: None,
                    ports: vec![
                        BotNodePortDescriptor {
                            port_id: "input".into(),
                            title: "Input".into(),
                            direction: BotNodePortDirection::Input,
                            event_type: event.clone(),
                            required: false,
                        },
                        BotNodePortDescriptor {
                            port_id: "event".into(),
                            title: "Event".into(),
                            direction: BotNodePortDirection::Output,
                            event_type: event,
                            required: false,
                        },
                    ],
                    config_schema: serde_json::json!({
                        "type": "object",
                        "additionalProperties": false
                    }),
                },
            )]),
        }
    }

    fn flow(id: &str) -> BotFlowDocument {
        BotFlowDocument {
            flow_id: id.into(),
            name: id.into(),
            nodes: vec![BotFlowNode {
                node_id: "source".into(),
                node_type_id: "test.source".into(),
                node_type_version: 1,
                config: serde_json::json!({}),
                source: Some(BotFlowSourceSelector {
                    protocol_id: "test.ingress".into(),
                    event_type: None,
                }),
                position: BotFlowNodePosition::default(),
            }],
            edges: Vec::new(),
        }
    }

    fn service(registry: Arc<BotFlowRegistry>) -> Arc<ConfigService> {
        let providers = Arc::new(ConfigProviderRegistry::default());
        providers
            .register(Arc::new(BotFlowConfigProvider::new(registry)))
            .unwrap();
        Arc::new(
            ConfigService::new(providers, Arc::new(InMemoryConfigRepository::default())).unwrap(),
        )
    }

    fn apply(service: Arc<ConfigService>, expected: u64, flow: BotFlowDocument) {
        futures_executor::block_on(async move {
            service
                .apply(
                    BOT_FLOW_CONFIG_PROVIDER_ID,
                    ConfigApplyRequest {
                        candidate: BotFlowConfigProvider::encode(&flow).unwrap(),
                        expected_revision: ConfigRevision(expected),
                        dry_run: false,
                    },
                    ConfigContext::global(),
                    &[capability::VALUE_WRITE.into(), capability::APPLY.into()],
                )
                .await
                .unwrap();
        });
    }

    #[test]
    fn config_revision_swaps_snapshot_without_mutating_inflight_arc() {
        let registry = Arc::new(BotFlowRegistry::new(catalog()));
        let service = service(registry.clone());
        apply(service.clone(), 0, flow("first"));
        let inflight = registry.active();
        apply(service, 1, flow("second"));

        assert_eq!(inflight.revision, 1);
        assert_eq!(inflight.flow.flow_id, "first");
        assert_eq!(registry.active().revision, 2);
        assert_eq!(registry.active().flow.flow_id, "second");
    }

    #[test]
    fn validation_rejects_cycles_and_preserves_error_location() {
        let registry = BotFlowRegistry::new(catalog());
        let mut candidate = flow("cyclic");
        candidate.edges.push(BotFlowEdge {
            edge_id: "loop".into(),
            from_node_id: "source".into(),
            from_port_id: "event".into(),
            to_node_id: "source".into(),
            to_port_id: "input".into(),
            kind: BotFlowEdgeKind::Event,
        });

        let result = registry.validate(&candidate);
        assert!(!result.valid);
        assert!(result.issues.iter().any(|issue| issue.code == "flow.cycle"));
    }

    #[test]
    fn empty_graph_is_valid_and_legacy_single_flow_upgrades() {
        let registry = BotFlowRegistry::new(catalog());
        assert!(registry.validate(&BotFlowDocument::default()).valid);

        let upgraded = BotFlowConfigProvider::decode(&ConfigValue::from_json(
            &serde_json::json!({ "flows": [flow("legacy")] }),
        ))
        .unwrap();
        assert_eq!(upgraded.flow_id, "legacy");

        let empty = BotFlowConfigProvider::decode(&ConfigValue::from_json(
            &serde_json::json!({ "flows": [] }),
        ))
        .unwrap();
        assert_eq!(empty, BotFlowDocument::default());
    }

    #[test]
    fn decode_rejects_multiple_legacy_flows() {
        let error = BotFlowConfigProvider::decode(&ConfigValue::from_json(&serde_json::json!({
            "flows": [flow("left"), flow("right")]
        })))
        .unwrap_err();
        assert!(error.to_string().contains("exactly one flow document"));
    }

    #[test]
    fn load_plan_activation_clears_incompatible_active_flow() {
        let registry = BotFlowRegistry::with_snapshot(
            catalog(),
            BotFlowSnapshot {
                revision: 4,
                flow: flow("live"),
            },
        )
        .unwrap();
        let plan = RuntimeLoadPlan {
            lock_version: 1,
            core_api_version: "1".into(),
            profile_id: "test".into(),
            profile_hash: "hash".into(),
            registry_generation: 1,
            plugins: Vec::new(),
            load_order: Vec::new(),
            runner_bindings: BTreeMap::new(),
            plugin_deployments: BTreeMap::new(),
            observability: Default::default(),
            capability_graph: Default::default(),
            contract_surfaces: Vec::new(),
        };
        registry.validate_load_plan(&plan).unwrap();
        registry.activate_load_plan(&plan).unwrap();
        assert!(registry.catalog().is_empty());
        assert_eq!(registry.active().revision, 0);
        assert_eq!(registry.active().flow, BotFlowDocument::default());
    }
}
