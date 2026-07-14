use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use mutsuki_distributed_contracts::{
    AcceptanceMode, DataSensitivity, DistributionMode, NodeTrustLevel, RecoveryTier,
    ResultVerificationPolicy,
};
use mutsuki_runtime_contracts::{
    CachePolicy, ExecutionMobility, LatencyClass, PartialResultPolicy, QualityPolicy, RetrySafety,
};
use serde::Deserialize;

pub const DISTRIBUTED_HOST_REVISION: &str = "76f3745fe3c4387035fe6b0a3031f9dfa861f8df";
pub const DISTRIBUTED_HOST_ARTIFACT: &str = "mutsuki-distributed-host";

#[derive(Clone, Debug, PartialEq)]
pub struct DistributionConfigError {
    pub code: &'static str,
    pub message: String,
}

impl DistributionConfigError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for DistributionConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DistributionConfigError {}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProductDistributionConfig {
    #[serde(default)]
    pub mode: DistributionMode,
    pub deployment: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DistributionPlan {
    pub mode: DistributionMode,
    pub deployment_path: Option<PathBuf>,
    pub deployment: Option<DistributedDeployment>,
    pub task_policies: Vec<TaskPolicy>,
}

impl DistributionPlan {
    pub fn disabled() -> Self {
        Self {
            mode: DistributionMode::Disabled,
            deployment_path: None,
            deployment: None,
            task_policies: Vec::new(),
        }
    }

    /// The template never supervises the external DistributedHost process.
    pub const fn template_managed_processes(&self) -> usize {
        0
    }

    /// The template's local ServiceRuntime never opens a distributed listener.
    pub const fn template_opens_distributed_network(&self) -> bool {
        false
    }

    pub const fn requires_external_service(&self) -> bool {
        !matches!(self.mode, DistributionMode::Disabled)
    }

    pub fn unavailable_decision(
        &self,
        policy: &TaskPolicy,
    ) -> Result<UnavailableDecision, DistributionConfigError> {
        match policy.acceptance {
            AcceptanceMode::Durable | AcceptanceMode::Critical { .. } => {
                Err(DistributionConfigError::new(
                    "distribution.durability_unavailable",
                    format!(
                        "task policy `{}` requires {:?}; local execution cannot claim durable acceptance",
                        policy.name, policy.acceptance
                    ),
                ))
            }
            AcceptanceMode::Fast => match policy.fallback {
                FallbackPolicy::LocalOnly => Ok(UnavailableDecision::RunLocal),
                FallbackPolicy::Reject => Err(DistributionConfigError::new(
                    "distribution.unavailable",
                    format!(
                        "task policy `{}` explicitly rejects execution while DistributedHost is unavailable",
                        policy.name
                    ),
                )),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnavailableDecision {
    RunLocal,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DistributedDeployment {
    pub schema_version: u16,
    pub external_service: ExternalService,
    pub topology: Topology,
    pub channels: Channels,
    pub budgets: ResourceBudgets,
    pub policy_catalog: PathBuf,
    pub observability: Observability,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExternalService {
    pub artifact: String,
    pub revision: String,
    pub local_host_control_endpoint: String,
    pub control_secret_key: String,
    pub identity_secret_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TopologyKind {
    SingleNode,
    ControllerWorker,
    HaThreeVotersWorker,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    Voter,
    Worker,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    pub kind: TopologyKind,
    pub nodes: Vec<DeploymentNode>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeploymentNode {
    pub id: String,
    pub roles: BTreeSet<NodeRole>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Channels {
    pub control: AuthenticatedChannel,
    pub data: DataChannel,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedChannel {
    pub endpoint: String,
    pub authenticated: bool,
    pub encrypted: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DataChannel {
    pub endpoint: String,
    pub authenticated: bool,
    pub encrypted: bool,
    pub direct_worker_transfer: bool,
    pub leader_proxy: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResourceBudgets {
    pub cpu_units: u32,
    pub memory_bytes: u64,
    pub vram_bytes: u64,
    pub network_bytes_per_second: u64,
    pub transfer_concurrency: u32,
    pub checkpoint_bytes_per_second: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Observability {
    pub local_service_health: String,
    pub cluster_health_endpoint: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    LocalOnly,
    Reject,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskPolicyCatalog {
    pub schema_version: u16,
    pub task: Vec<TaskPolicy>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskPolicy {
    pub name: String,
    pub mobility: ExecutionMobility,
    pub recovery: RecoveryTier,
    pub latency: LatencyClass,
    pub acceptance: AcceptanceMode,
    pub effect: RetrySafety,
    pub quality: QualityPolicy,
    pub cache: CachePolicy,
    pub partial_results: PartialResultPolicy,
    pub fallback: FallbackPolicy,
    pub trust: TaskTrustPolicyConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskTrustPolicyConfig {
    pub sensitivity: DataSensitivity,
    pub minimum_trust: NodeTrustLevel,
    pub verification: ResultVerificationPolicy,
    pub allow_external_workers: bool,
    pub allow_persistent_cache: bool,
    pub require_attestation: bool,
}

pub fn load_distribution_plan(
    product_config_path: &Path,
) -> Result<DistributionPlan, DistributionConfigError> {
    let source = fs::read_to_string(product_config_path).map_err(|error| {
        DistributionConfigError::new(
            "distribution.product_config_unreadable",
            format!("{}: {error}", product_config_path.display()),
        )
    })?;
    let product: toml::Value = toml::from_str(&source).map_err(|error| {
        DistributionConfigError::new("distribution.product_config_invalid", error.to_string())
    })?;
    let selection: ProductDistributionConfig = product
        .get("distribution")
        .cloned()
        .map(toml::Value::try_into)
        .transpose()
        .map_err(|error| {
            DistributionConfigError::new("distribution.selection_invalid", error.to_string())
        })?
        .unwrap_or_default();

    if selection.mode == DistributionMode::Disabled {
        if selection.deployment.is_some() {
            return Err(DistributionConfigError::new(
                "distribution.disabled_has_deployment",
                "disabled mode must not name a deployment",
            ));
        }
        return Ok(DistributionPlan::disabled());
    }

    let relative_path = selection.deployment.ok_or_else(|| {
        DistributionConfigError::new(
            "distribution.deployment_required",
            format!(
                "{:?} mode requires an explicit deployment file",
                selection.mode
            ),
        )
    })?;
    let deployment_path = resolve_relative_to(product_config_path, &relative_path);
    let deployment_source = fs::read_to_string(&deployment_path).map_err(|error| {
        DistributionConfigError::new(
            "distribution.deployment_unreadable",
            format!("{}: {error}", deployment_path.display()),
        )
    })?;
    let deployment: DistributedDeployment =
        toml::from_str(&deployment_source).map_err(|error| {
            DistributionConfigError::new("distribution.deployment_invalid", error.to_string())
        })?;
    validate_deployment(&deployment)?;

    let policy_path = resolve_relative_to(&deployment_path, &deployment.policy_catalog);
    let policy_source = fs::read_to_string(&policy_path).map_err(|error| {
        DistributionConfigError::new(
            "distribution.policy_catalog_unreadable",
            format!("{}: {error}", policy_path.display()),
        )
    })?;
    let catalog: TaskPolicyCatalog = toml::from_str(&policy_source).map_err(|error| {
        DistributionConfigError::new("distribution.policy_catalog_invalid", error.to_string())
    })?;
    validate_task_policies(&catalog)?;

    Ok(DistributionPlan {
        mode: selection.mode,
        deployment_path: Some(deployment_path),
        deployment: Some(deployment),
        task_policies: catalog.task,
    })
}

fn resolve_relative_to(owner_file: &Path, referenced: &Path) -> PathBuf {
    if referenced.is_absolute() {
        referenced.to_owned()
    } else {
        owner_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(referenced)
    }
}

fn validate_deployment(deployment: &DistributedDeployment) -> Result<(), DistributionConfigError> {
    if deployment.schema_version != 1 {
        return Err(DistributionConfigError::new(
            "distribution.schema_unsupported",
            format!(
                "deployment schema {} is unsupported",
                deployment.schema_version
            ),
        ));
    }
    if deployment.external_service.artifact != DISTRIBUTED_HOST_ARTIFACT {
        return Err(DistributionConfigError::new(
            "distribution.artifact_invalid",
            format!(
                "expected external artifact `{DISTRIBUTED_HOST_ARTIFACT}`, got `{}`",
                deployment.external_service.artifact
            ),
        ));
    }
    if deployment.external_service.revision != DISTRIBUTED_HOST_REVISION {
        return Err(DistributionConfigError::new(
            "distribution.revision_mismatch",
            format!(
                "deployment revision `{}` does not match pinned contracts `{DISTRIBUTED_HOST_REVISION}`",
                deployment.external_service.revision
            ),
        ));
    }
    if deployment
        .external_service
        .local_host_control_endpoint
        .trim()
        .is_empty()
    {
        return Err(DistributionConfigError::new(
            "distribution.local_host_endpoint_required",
            "the external sidecar must explicitly connect to the ordinary local ServiceHost control endpoint",
        ));
    }
    validate_secret_key(
        "control_secret_key",
        &deployment.external_service.control_secret_key,
    )?;
    validate_secret_key(
        "identity_secret_key",
        &deployment.external_service.identity_secret_key,
    )?;
    validate_topology(&deployment.topology)?;
    validate_channels(&deployment.channels)?;
    validate_budgets(deployment.budgets)?;
    if deployment
        .observability
        .local_service_health
        .trim()
        .is_empty()
        || deployment
            .observability
            .cluster_health_endpoint
            .trim()
            .is_empty()
    {
        return Err(DistributionConfigError::new(
            "distribution.observability_invalid",
            "local and cluster health sources must both be explicit",
        ));
    }
    Ok(())
}

fn validate_secret_key(name: &str, value: &str) -> Result<(), DistributionConfigError> {
    let valid = value.ends_with("_KEY")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        && value.bytes().any(|byte| byte.is_ascii_uppercase());
    if valid {
        Ok(())
    } else {
        Err(DistributionConfigError::new(
            "distribution.secret_reference_invalid",
            format!(
                "{name} must be an uppercase secret key reference ending in `_KEY`, never secret material"
            ),
        ))
    }
}

fn validate_topology(topology: &Topology) -> Result<(), DistributionConfigError> {
    if topology.nodes.is_empty() {
        return Err(DistributionConfigError::new(
            "distribution.topology_empty",
            "at least one node is required",
        ));
    }
    let ids: BTreeSet<_> = topology.nodes.iter().map(|node| node.id.as_str()).collect();
    if ids.len() != topology.nodes.len() || ids.iter().any(|id| id.trim().is_empty()) {
        return Err(DistributionConfigError::new(
            "distribution.node_identity_invalid",
            "node identifiers must be non-empty and unique",
        ));
    }
    if topology.nodes.iter().any(|node| node.roles.is_empty()) {
        return Err(DistributionConfigError::new(
            "distribution.node_role_missing",
            "every node must explicitly declare at least one role",
        ));
    }
    let voters = topology
        .nodes
        .iter()
        .filter(|node| node.roles.contains(&NodeRole::Voter))
        .count();
    let workers = topology
        .nodes
        .iter()
        .filter(|node| node.roles.contains(&NodeRole::Worker))
        .count();
    let matches_kind = match topology.kind {
        TopologyKind::SingleNode => topology.nodes.len() == 1 && voters == 1 && workers == 1,
        TopologyKind::ControllerWorker => voters == 1 && workers >= 1,
        TopologyKind::HaThreeVotersWorker => voters == 3 && workers >= 1,
    };
    if matches_kind {
        Ok(())
    } else {
        Err(DistributionConfigError::new(
            "distribution.topology_shape_invalid",
            format!(
                "{:?} topology has {} nodes, {voters} voters and {workers} workers",
                topology.kind,
                topology.nodes.len()
            ),
        ))
    }
}

fn validate_channels(channels: &Channels) -> Result<(), DistributionConfigError> {
    if channels.control.endpoint.trim().is_empty()
        || channels.data.endpoint.trim().is_empty()
        || channels.control.endpoint == channels.data.endpoint
    {
        return Err(DistributionConfigError::new(
            "distribution.channel_separation_required",
            "control and direct-data endpoints must be non-empty and distinct",
        ));
    }
    if !channels.control.authenticated
        || !channels.control.encrypted
        || !channels.data.authenticated
        || !channels.data.encrypted
    {
        return Err(DistributionConfigError::new(
            "distribution.channel_security_required",
            "control and data channels must be authenticated and encrypted",
        ));
    }
    if !channels.data.direct_worker_transfer || channels.data.leader_proxy {
        return Err(DistributionConfigError::new(
            "distribution.leader_data_proxy_forbidden",
            "large data must transfer directly between origin and Worker; Leader proxy is forbidden",
        ));
    }
    Ok(())
}

fn validate_budgets(budgets: ResourceBudgets) -> Result<(), DistributionConfigError> {
    if budgets.cpu_units == 0
        || budgets.memory_bytes == 0
        || budgets.network_bytes_per_second == 0
        || budgets.transfer_concurrency == 0
        || budgets.checkpoint_bytes_per_second == 0
    {
        return Err(DistributionConfigError::new(
            "distribution.budget_invalid",
            "CPU, memory, network, transfer concurrency and checkpoint budgets must be positive; VRAM may be zero",
        ));
    }
    Ok(())
}

fn validate_task_policies(catalog: &TaskPolicyCatalog) -> Result<(), DistributionConfigError> {
    if catalog.schema_version != 1 || catalog.task.is_empty() {
        return Err(DistributionConfigError::new(
            "distribution.policy_catalog_invalid",
            "policy catalog schema must be 1 and contain at least one task policy",
        ));
    }
    let names: BTreeSet<_> = catalog
        .task
        .iter()
        .map(|policy| policy.name.as_str())
        .collect();
    if names.len() != catalog.task.len() || names.iter().any(|name| name.trim().is_empty()) {
        return Err(DistributionConfigError::new(
            "distribution.policy_name_invalid",
            "task policy names must be non-empty and unique",
        ));
    }
    for policy in &catalog.task {
        if policy.quality.minimum_level > policy.quality.requested_level {
            return Err(DistributionConfigError::new(
                "distribution.quality_policy_invalid",
                format!(
                    "task policy `{}` has minimum quality above requested quality",
                    policy.name
                ),
            ));
        }
        let sensitive = matches!(
            policy.trust.sensitivity,
            DataSensitivity::Confidential
                | DataSensitivity::Restricted
                | DataSensitivity::Credential
        );
        if sensitive && policy.trust.minimum_trust < NodeTrustLevel::Managed {
            return Err(DistributionConfigError::new(
                "distribution.sensitive_task_trust_invalid",
                format!(
                    "task policy `{}` requires sensitive data but permits nodes below managed trust",
                    policy.name
                ),
            ));
        }
        if matches!(policy.mobility, ExecutionMobility::LocalOnly)
            && !matches!(policy.fallback, FallbackPolicy::LocalOnly)
        {
            return Err(DistributionConfigError::new(
                "distribution.local_policy_invalid",
                format!(
                    "local-only task policy `{}` must explicitly use local_only fallback",
                    policy.name
                ),
            ));
        }
        if matches!(policy.recovery, RecoveryTier::Checkpointed)
            && !matches!(policy.mobility, ExecutionMobility::Checkpointable)
        {
            return Err(DistributionConfigError::new(
                "distribution.checkpoint_policy_invalid",
                format!(
                    "checkpointed task policy `{}` must be checkpointable",
                    policy.name
                ),
            ));
        }
    }
    Ok(())
}
