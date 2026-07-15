use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use mutsuki_distributed_contracts::DistributionMode;
use serde::Deserialize;
use serde::de::DeserializeOwned;

const DISTRIBUTED_HOST_REVISION: &str = "76f3745fe3c4387035fe6b0a3031f9dfa861f8df";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DistributionConfigError {
    pub code: &'static str,
    message: String,
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

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DistributionSelection {
    #[serde(default)]
    mode: DistributionMode,
    deployment: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deployment {
    schema_version: u16,
    external_service: ExternalService,
    topology: Topology,
    channels: Channels,
    budgets: Budgets,
    policy_catalog: PathBuf,
    observability: Observability,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalService {
    artifact: String,
    revision: String,
    local_host_control_endpoint: String,
    control_secret_key: String,
    identity_secret_key: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TopologyKind {
    SingleNode,
    ControllerWorker,
    HaThreeVotersWorker,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum NodeRole {
    Voter,
    Worker,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Topology {
    kind: TopologyKind,
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    id: String,
    roles: Vec<NodeRole>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Channels {
    control: Channel,
    data: DataChannel,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Channel {
    endpoint: String,
    authenticated: bool,
    encrypted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DataChannel {
    endpoint: String,
    authenticated: bool,
    encrypted: bool,
    direct_worker_transfer: bool,
    leader_proxy: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Budgets {
    cpu_units: u32,
    memory_bytes: u64,
    vram_bytes: u64,
    network_bytes_per_second: u64,
    transfer_concurrency: u32,
    checkpoint_bytes_per_second: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observability {
    local_service_health: String,
    cluster_health_endpoint: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyCatalog {
    schema_version: u16,
    task: Vec<toml::Value>,
}

/// Validate only the product-owned deployment boundary. DistributedHost owns
/// scheduling, fallback, recovery and trust semantics.
pub fn validate_distribution_config(
    product_config_path: &Path,
) -> Result<DistributionMode, DistributionConfigError> {
    let product: toml::Value = load_toml(
        product_config_path,
        "distribution.product_config_unreadable",
        "distribution.product_config_invalid",
    )?;
    let selection: DistributionSelection = product
        .get("distribution")
        .cloned()
        .map(toml::Value::try_into)
        .transpose()
        .map_err(|error| {
            DistributionConfigError::new("distribution.selection_invalid", error.to_string())
        })?
        .unwrap_or_default();

    if selection.mode == DistributionMode::Disabled {
        require(
            selection.deployment.is_none(),
            "distribution.disabled_has_deployment",
            "disabled mode must not name a deployment",
        )?;
        return Ok(selection.mode);
    }

    let deployment_ref = selection.deployment.ok_or_else(|| {
        DistributionConfigError::new(
            "distribution.deployment_required",
            "enabled distribution requires an explicit deployment file",
        )
    })?;
    let deployment_path = resolve(product_config_path, &deployment_ref);
    let deployment: Deployment = load_toml(
        &deployment_path,
        "distribution.deployment_unreadable",
        "distribution.deployment_invalid",
    )?;
    validate_deployment(&deployment_path, &deployment)?;
    Ok(selection.mode)
}

fn validate_deployment(
    deployment_path: &Path,
    deployment: &Deployment,
) -> Result<(), DistributionConfigError> {
    require(
        deployment.schema_version == 1,
        "distribution.deployment_invalid",
        "deployment schema_version must be 1",
    )?;
    require(
        deployment.external_service.artifact == "mutsuki-distributed-host"
            && deployment.external_service.revision == DISTRIBUTED_HOST_REVISION,
        "distribution.revision_mismatch",
        "deployment artifact and revision must match the pinned DistributedHost",
    )?;
    require(
        !deployment
            .external_service
            .local_host_control_endpoint
            .trim()
            .is_empty(),
        "distribution.local_host_endpoint_required",
        "the external sidecar requires a local ServiceHost control endpoint",
    )?;
    for secret_key in [
        &deployment.external_service.control_secret_key,
        &deployment.external_service.identity_secret_key,
    ] {
        require(
            is_secret_reference(secret_key),
            "distribution.secret_reference_invalid",
            "sidecar secrets must be uppercase key references ending in _KEY",
        )?;
    }
    validate_topology(&deployment.topology)?;

    let control = &deployment.channels.control;
    let data = &deployment.channels.data;
    require(
        !control.endpoint.trim().is_empty()
            && !data.endpoint.trim().is_empty()
            && control.endpoint != data.endpoint,
        "distribution.channel_separation_required",
        "control and data endpoints must be explicit and distinct",
    )?;
    require(
        control.authenticated
            && control.encrypted
            && data.authenticated
            && data.encrypted
            && data.direct_worker_transfer
            && !data.leader_proxy,
        "distribution.channel_policy_invalid",
        "channels must be authenticated/encrypted and data must bypass the Leader",
    )?;

    let budgets = &deployment.budgets;
    let _vram_budget_may_be_zero = budgets.vram_bytes;
    require(
        budgets.cpu_units > 0
            && budgets.memory_bytes > 0
            && budgets.network_bytes_per_second > 0
            && budgets.transfer_concurrency > 0
            && budgets.checkpoint_bytes_per_second > 0,
        "distribution.budget_invalid",
        "CPU, memory, network, transfer and checkpoint budgets must be positive",
    )?;
    require(
        !deployment
            .observability
            .local_service_health
            .trim()
            .is_empty()
            && !deployment
                .observability
                .cluster_health_endpoint
                .trim()
                .is_empty(),
        "distribution.observability_invalid",
        "local and cluster health sources must both be explicit",
    )?;

    let policy_path = resolve(deployment_path, &deployment.policy_catalog);
    let catalog: PolicyCatalog = load_toml(
        &policy_path,
        "distribution.policy_catalog_unreadable",
        "distribution.policy_catalog_invalid",
    )?;
    require(
        catalog.schema_version == 1 && !catalog.task.is_empty(),
        "distribution.policy_catalog_invalid",
        "policy catalog schema_version must be 1 and contain task examples",
    )
}

fn validate_topology(topology: &Topology) -> Result<(), DistributionConfigError> {
    let ids: BTreeSet<_> = topology.nodes.iter().map(|node| node.id.as_str()).collect();
    require(
        !topology.nodes.is_empty()
            && ids.len() == topology.nodes.len()
            && ids.iter().all(|id| !id.trim().is_empty())
            && topology.nodes.iter().all(|node| !node.roles.is_empty()),
        "distribution.topology_invalid",
        "nodes must have unique non-empty identifiers and explicit roles",
    )?;
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
    let valid = match topology.kind {
        TopologyKind::SingleNode => topology.nodes.len() == 1 && voters == 1 && workers == 1,
        TopologyKind::ControllerWorker => voters == 1 && workers >= 1,
        TopologyKind::HaThreeVotersWorker => voters == 3 && workers >= 1,
    };
    require(
        valid,
        "distribution.topology_invalid",
        "node roles do not match the selected topology",
    )
}

fn is_secret_reference(value: &str) -> bool {
    value.ends_with("_KEY")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn resolve(owner_file: &Path, referenced: &Path) -> PathBuf {
    if referenced.is_absolute() {
        referenced.to_owned()
    } else {
        owner_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(referenced)
    }
}

fn load_toml<T: DeserializeOwned>(
    path: &Path,
    unreadable_code: &'static str,
    invalid_code: &'static str,
) -> Result<T, DistributionConfigError> {
    let source = fs::read_to_string(path).map_err(|error| {
        DistributionConfigError::new(unreadable_code, format!("{}: {error}", path.display()))
    })?;
    toml::from_str(&source)
        .map_err(|error| DistributionConfigError::new(invalid_code, error.to_string()))
}

fn require(
    condition: bool,
    code: &'static str,
    message: &'static str,
) -> Result<(), DistributionConfigError> {
    if condition {
        Ok(())
    } else {
        Err(DistributionConfigError::new(code, message))
    }
}
