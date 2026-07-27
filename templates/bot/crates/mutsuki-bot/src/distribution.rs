use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use mutsuki_distributed_contracts::{
    CapabilityMaturity, DISTRIBUTED_CAPABILITY_SCHEMA_VERSION, DISTRIBUTED_HOST_RELEASE,
    DISTRIBUTED_HOST_REVISION, DISTRIBUTED_PROTOCOL_MAJOR, DistributedFeature, DistributionMode,
    NodeId, SidecarCapabilityProof,
};
use mutsuki_distributed_control_client::DistributedControlClient;
use mutsuki_service_config::ServiceConfig;
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RequestedAcceptance {
    #[default]
    Fast,
    Durable,
    Critical,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LocalFallback {
    #[default]
    Reject,
    LocalDegraded,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DistributionSelection {
    #[serde(default)]
    mode: DistributionMode,
    deployment: Option<PathBuf>,
    #[serde(default)]
    acceptance: RequestedAcceptance,
    #[serde(default)]
    fallback: LocalFallback,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deployment {
    schema_version: u16,
    distributed_host_release: String,
    capability_level: CapabilityMaturity,
    required_features: BTreeSet<DistributedFeature>,
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
    sidecar_control_endpoint: String,
    sidecar_control_client_node: String,
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
    #[serde(rename = "vram_bytes")]
    _vram_bytes: u64,
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

struct ValidatedDistribution {
    selection: DistributionSelection,
    deployment: Option<Deployment>,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct DistributionHealth {
    mode: &'static str,
    state: &'static str,
    execution: &'static str,
    remote_execution: bool,
    fallback: &'static str,
    sidecar_revision: Option<String>,
    last_error_code: Option<String>,
}

pub struct DistributionRuntime {
    health: Arc<RwLock<DistributionHealth>>,
    monitor: Option<MonitorConfig>,
}

struct MonitorConfig {
    client: Arc<DistributedControlClient>,
    expected_release: String,
    expected_revision: String,
    capability_level: CapabilityMaturity,
    required_features: BTreeSet<DistributedFeature>,
    fallback: LocalFallback,
}

pub struct DistributionMonitor(tokio::task::JoinHandle<()>);

impl Drop for DistributionMonitor {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl DistributionRuntime {
    fn local(mode: &'static str, state: &'static str) -> Self {
        Self {
            health: Arc::new(RwLock::new(DistributionHealth {
                mode,
                state,
                execution: "local",
                remote_execution: false,
                fallback: "reject",
                sidecar_revision: None,
                last_error_code: None,
            })),
            monitor: None,
        }
    }

    pub fn health_snapshot(&self) -> serde_json::Value {
        health_snapshot(&self.health)
    }

    pub fn attach_health_probe(&self, builder: ServiceRuntimeBuilder) -> ServiceRuntimeBuilder {
        let health = self.health.clone();
        builder.register_health_probe("distribution", move || health_snapshot(&health))
    }

    pub fn start_monitor(&mut self) -> Option<DistributionMonitor> {
        let config = self.monitor.take()?;
        let health = self.health.clone();
        Some(DistributionMonitor(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let result = probe_sidecar(
                    &config.client,
                    &config.expected_release,
                    &config.expected_revision,
                    config.capability_level,
                    &config.required_features,
                )
                .await;
                *health.write().expect("distribution health write lock") = health_after_probe(
                    result
                        .as_ref()
                        .ok()
                        .map(|proof| proof.distributed_host_revision.clone()),
                    result.as_ref().err(),
                    config.fallback,
                );
            }
        })))
    }
}

fn health_snapshot(health: &RwLock<DistributionHealth>) -> serde_json::Value {
    let health = health.read().expect("distribution health read lock");
    serde_json::to_value(&*health).expect("distribution health serializes")
}

/// Static validation performs no network access and constructs no background task.
pub fn validate_distribution_config(
    product_config_path: &Path,
) -> Result<DistributionMode, DistributionConfigError> {
    Ok(load_distribution(product_config_path)?.selection.mode)
}

/// Validate, authenticate, and prove the selected distribution capability before local boot.
pub async fn prepare_distribution(
    product_config_path: &Path,
    service: &ServiceConfig,
) -> Result<DistributionRuntime, DistributionConfigError> {
    let validated = load_distribution(product_config_path)?;
    match validated.selection.mode {
        DistributionMode::Disabled => Ok(DistributionRuntime::local("disabled", "disabled")),
        DistributionMode::LocalObservable => Ok(DistributionRuntime::local(
            "local_observable",
            "observing_local_only",
        )),
        DistributionMode::Clustered => {
            let deployment = validated
                .deployment
                .expect("clustered validation requires deployment");
            let secret = service
                .secret(&deployment.external_service.control_secret_key)
                .filter(|secret| !secret.trim().is_empty())
                .ok_or_else(|| {
                    DistributionConfigError::new(
                        "distribution.control_secret_missing",
                        "sidecar control secret reference could not be resolved",
                    )
                })?;
            let address = deployment
                .external_service
                .sidecar_control_endpoint
                .strip_prefix(LINK_LOCAL_PREFIX)
                .expect("validated sidecar endpoint");
            let client = Arc::new(
                DistributedControlClient::new(
                    NodeId(
                        deployment
                            .external_service
                            .sidecar_control_client_node
                            .clone(),
                    ),
                    address,
                    Arc::from(secret.into_bytes()),
                    Duration::from_secs(2),
                )
                .map_err(|error| {
                    DistributionConfigError::new(
                        "distribution.control_client_invalid",
                        error.to_string(),
                    )
                })?,
            );
            let result = probe_sidecar(
                &client,
                &deployment.distributed_host_release,
                &deployment.external_service.revision,
                deployment.capability_level,
                &deployment.required_features,
            )
            .await;
            let fallback = validated.selection.fallback;
            let initial_health = match &result {
                Ok(proof) => health_after_probe(
                    Some(proof.distributed_host_revision.clone()),
                    None,
                    fallback,
                ),
                Err(error) if fallback == LocalFallback::LocalDegraded => {
                    health_after_probe(None, Some(error), fallback)
                }
                Err(error) => return Err(error.clone()),
            };
            Ok(DistributionRuntime {
                health: Arc::new(RwLock::new(initial_health)),
                monitor: Some(MonitorConfig {
                    client,
                    expected_release: deployment.distributed_host_release,
                    expected_revision: deployment.external_service.revision,
                    capability_level: deployment.capability_level,
                    required_features: deployment.required_features,
                    fallback,
                }),
            })
        }
    }
}

async fn probe_sidecar(
    client: &DistributedControlClient,
    expected_release: &str,
    expected_revision: &str,
    capability_level: CapabilityMaturity,
    required_features: &BTreeSet<DistributedFeature>,
) -> Result<SidecarCapabilityProof, DistributionConfigError> {
    let proof = client.capabilities().await.map_err(|error| {
        DistributionConfigError::new("distribution.sidecar_unavailable", error.to_string())
    })?;
    validate_capability_proof(
        &proof,
        expected_release,
        expected_revision,
        capability_level,
        required_features,
    )?;
    let health = client.health().await.map_err(|error| {
        DistributionConfigError::new("distribution.sidecar_unavailable", error.to_string())
    })?;
    require(
        health == "healthy",
        "distribution.sidecar_unhealthy",
        "sidecar health is not healthy",
    )?;
    Ok(proof)
}

fn health_after_probe(
    revision: Option<String>,
    error: Option<&DistributionConfigError>,
    fallback: LocalFallback,
) -> DistributionHealth {
    match error {
        None => DistributionHealth {
            mode: "clustered",
            state: "healthy",
            execution: "clustered",
            remote_execution: true,
            fallback: fallback_name(fallback),
            sidecar_revision: revision,
            last_error_code: None,
        },
        Some(error) => {
            let (state, execution) = match fallback {
                LocalFallback::Reject => ("unavailable", "clustered_unavailable"),
                LocalFallback::LocalDegraded => ("degraded", "local_fallback"),
            };
            DistributionHealth {
                mode: "clustered",
                state,
                execution,
                remote_execution: false,
                fallback: fallback_name(fallback),
                sidecar_revision: revision,
                last_error_code: Some(error.code.into()),
            }
        }
    }
}

const fn fallback_name(fallback: LocalFallback) -> &'static str {
    match fallback {
        LocalFallback::Reject => "reject",
        LocalFallback::LocalDegraded => "local_degraded",
    }
}

fn load_distribution(
    product_config_path: &Path,
) -> Result<ValidatedDistribution, DistributionConfigError> {
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
            selection.deployment.is_none()
                && selection.acceptance == RequestedAcceptance::Fast
                && selection.fallback == LocalFallback::Reject,
            "distribution.disabled_has_configuration",
            "disabled mode must not name deployment, acceptance, or fallback options",
        )?;
        return Ok(ValidatedDistribution {
            selection,
            deployment: None,
        });
    }
    if selection.mode == DistributionMode::LocalObservable {
        require(
            selection.fallback == LocalFallback::Reject,
            "distribution.local_observable_fallback_invalid",
            "local-observable mode never performs remote execution or fallback",
        )?;
    }
    require(
        selection.fallback != LocalFallback::LocalDegraded
            || selection.acceptance == RequestedAcceptance::Fast,
        "distribution.fallback_invalid",
        "only Fast acceptance may explicitly use local_degraded fallback",
    )?;

    let deployment_ref = selection.deployment.clone().ok_or_else(|| {
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
    require(
        selection.fallback != LocalFallback::LocalDegraded
            || (!deployment
                .required_features
                .contains(&DistributedFeature::Durable)
                && !deployment
                    .required_features
                    .contains(&DistributedFeature::Critical)),
        "distribution.fallback_invalid",
        "local_degraded fallback cannot be combined with Durable or Critical feature requirements",
    )?;
    validate_deployment(
        &deployment_path,
        &deployment,
        selection.mode,
        selection.acceptance,
    )?;
    Ok(ValidatedDistribution {
        selection,
        deployment: Some(deployment),
    })
}

fn validate_capability_proof(
    proof: &SidecarCapabilityProof,
    expected_release: &str,
    expected_revision: &str,
    capability_level: CapabilityMaturity,
    required_features: &BTreeSet<DistributedFeature>,
) -> Result<(), DistributionConfigError> {
    require(
        proof.schema_version == DISTRIBUTED_CAPABILITY_SCHEMA_VERSION
            && proof.protocol_major == DISTRIBUTED_PROTOCOL_MAJOR,
        "distribution.capability_version_mismatch",
        "sidecar capability schema or protocol version is incompatible",
    )?;
    require(
        proof.distributed_host_release == expected_release
            && proof.distributed_host_revision == expected_revision,
        "distribution.revision_mismatch",
        "sidecar release and revision must match the active product pin",
    )?;
    require(
        maturity_rank(proof.capability_level) >= maturity_rank(capability_level),
        "distribution.experimental_unavailable",
        "sidecar aggregate capability level is insufficient",
    )?;
    require(
        required_features.iter().all(|feature| {
            proof
                .feature_proof
                .get(feature)
                .is_some_and(|maturity| maturity_rank(*maturity) >= maturity_rank(capability_level))
        }),
        "distribution.experimental_unavailable",
        "one or more required sidecar features are not deployable at the requested level",
    )
}

const fn maturity_rank(maturity: CapabilityMaturity) -> u8 {
    match maturity {
        CapabilityMaturity::Unavailable => 0,
        CapabilityMaturity::Contract => 1,
        CapabilityMaturity::ReferenceModel => 2,
        CapabilityMaturity::InProcessTest => 3,
        CapabilityMaturity::Deployable => 4,
        CapabilityMaturity::ProductionReady => 5,
    }
}

fn validate_deployment(
    deployment_path: &Path,
    deployment: &Deployment,
    mode: DistributionMode,
    acceptance: RequestedAcceptance,
) -> Result<(), DistributionConfigError> {
    require(
        deployment.schema_version == 2,
        "distribution.deployment_invalid",
        "deployment schema_version must be 2",
    )?;
    require(
        deployment.distributed_host_release == DISTRIBUTED_HOST_RELEASE
            && deployment.external_service.artifact == "mutsuki-distributed-host"
            && deployment.external_service.revision == DISTRIBUTED_HOST_REVISION,
        "distribution.revision_mismatch",
        "deployment release and revision must match the pinned DistributedHost",
    )?;
    require(
        maturity_rank(deployment.capability_level) >= maturity_rank(CapabilityMaturity::Deployable),
        "distribution.capability_level_invalid",
        "product deployment must require at least deployable sidecar capabilities",
    )?;
    require(
        deployment
            .required_features
            .contains(if mode == DistributionMode::LocalObservable {
                &DistributedFeature::LocalObservation
            } else {
                &DistributedFeature::Clustered
            }),
        "distribution.required_features_invalid",
        "enabled deployment must require capability proof for the selected mode",
    )?;
    let acceptance_features_present = match acceptance {
        RequestedAcceptance::Fast => true,
        RequestedAcceptance::Durable => deployment
            .required_features
            .contains(&DistributedFeature::Durable),
        RequestedAcceptance::Critical => {
            deployment
                .required_features
                .contains(&DistributedFeature::Durable)
                && deployment
                    .required_features
                    .contains(&DistributedFeature::Critical)
        }
    };
    require(
        acceptance_features_present,
        "distribution.required_features_invalid",
        "requested acceptance must have matching required feature proof",
    )?;
    require(
        !matches!(deployment.topology.kind, TopologyKind::HaThreeVotersWorker)
            || deployment
                .required_features
                .contains(&DistributedFeature::HighAvailability),
        "distribution.required_features_invalid",
        "HA topology must require high_availability capability proof",
    )?;
    require(
        is_present(&deployment.external_service.local_host_control_endpoint)
            && deployment
                .external_service
                .sidecar_control_endpoint
                .strip_prefix(LINK_LOCAL_PREFIX)
                .is_some_and(|address| !address.is_empty())
            && is_present(&deployment.external_service.sidecar_control_client_node),
        "distribution.control_endpoint_invalid",
        "local Host and authenticated sidecar control endpoints must be explicit",
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
        is_present(&control.endpoint)
            && is_present(&data.endpoint)
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
        is_present(&deployment.observability.local_service_health)
            && is_present(&deployment.observability.cluster_health_endpoint),
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

const LINK_LOCAL_PREFIX: &str = "link-local://";

fn is_present(value: &str) -> bool {
    !value.trim().is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn proof() -> SidecarCapabilityProof {
        SidecarCapabilityProof::current()
    }

    #[test]
    fn proof_rejects_version_revision_and_feature_mismatch() {
        let required = BTreeSet::from([DistributedFeature::Clustered]);
        assert!(
            validate_capability_proof(
                &proof(),
                DISTRIBUTED_HOST_RELEASE,
                DISTRIBUTED_HOST_REVISION,
                CapabilityMaturity::Deployable,
                &required,
            )
            .is_ok()
        );

        let mut incompatible = proof();
        incompatible.protocol_major += 1;
        assert_eq!(
            validate_capability_proof(
                &incompatible,
                DISTRIBUTED_HOST_RELEASE,
                DISTRIBUTED_HOST_REVISION,
                CapabilityMaturity::Deployable,
                &required,
            )
            .unwrap_err()
            .code,
            "distribution.capability_version_mismatch"
        );

        let mut wrong_revision = proof();
        wrong_revision.distributed_host_revision = "0".repeat(40);
        assert_eq!(
            validate_capability_proof(
                &wrong_revision,
                DISTRIBUTED_HOST_RELEASE,
                DISTRIBUTED_HOST_REVISION,
                CapabilityMaturity::Deployable,
                &required,
            )
            .unwrap_err()
            .code,
            "distribution.revision_mismatch"
        );

        for unavailable in [
            DistributedFeature::Durable,
            DistributedFeature::Critical,
            DistributedFeature::HighAvailability,
            DistributedFeature::Checkpoint,
            DistributedFeature::Trust,
        ] {
            let features = BTreeSet::from([DistributedFeature::Clustered, unavailable]);
            assert_eq!(
                validate_capability_proof(
                    &proof(),
                    DISTRIBUTED_HOST_RELEASE,
                    DISTRIBUTED_HOST_REVISION,
                    CapabilityMaturity::Deployable,
                    &features,
                )
                .unwrap_err()
                .code,
                "distribution.experimental_unavailable",
                "{unavailable:?} must remain fail closed until deployable"
            );
        }
    }

    #[test]
    fn disconnect_and_recovery_are_visible_without_silent_fallback() {
        let unavailable =
            DistributionConfigError::new("distribution.sidecar_unavailable", "transport closed");
        let failed = health_after_probe(None, Some(&unavailable), LocalFallback::Reject);
        assert_eq!(failed.state, "unavailable");
        assert_eq!(failed.execution, "clustered_unavailable");
        assert!(!failed.remote_execution);

        let recovered = health_after_probe(
            Some(DISTRIBUTED_HOST_REVISION.into()),
            None,
            LocalFallback::Reject,
        );
        assert_eq!(recovered.state, "healthy");
        assert_eq!(recovered.execution, "clustered");
        assert!(recovered.remote_execution);
    }

    #[test]
    fn explicit_fast_fallback_is_degraded_and_never_claims_remote_execution() {
        let unavailable =
            DistributionConfigError::new("distribution.sidecar_unavailable", "transport closed");
        let degraded = health_after_probe(None, Some(&unavailable), LocalFallback::LocalDegraded);
        assert_eq!(degraded.state, "degraded");
        assert_eq!(degraded.execution, "local_fallback");
        assert!(!degraded.remote_execution);
        assert_eq!(degraded.fallback, "local_degraded");
    }
}
