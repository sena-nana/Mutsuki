//! Mutsuki module upgrade catalog: release set pins, remote revision checks and upgrade planning.
//!
//! Does not perform runtime plugin registration; ABI artifacts must land under
//! `plugins/installed` and be selected via `[[plugins.configured]]` + LoadPlan.

mod execute;
mod release_set;
mod upgrade;

pub use execute::{
    StepStatus, UpgradeExecuteOptions, UpgradeExecuteReport, UpgradeStepResult,
    execute_module_upgrade, format_execute_cli_command,
};
pub use release_set::{ReleaseSetInfo, ReleaseSetRepository, load_release_set};
pub use upgrade::{
    FixtureRemoteHeadProvider, ModuleUpgradeSummary, RemoteHeadError, RemoteHeadProvider,
    ReqwestRemoteHeadProvider, UpgradePlan, UpgradeStatus, UpgradeStep, check_module_updates,
    default_abi_inventory_dir, plan_module_upgrade,
};

use std::path::Path;

use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("invalid release set: {0}")]
    ReleaseSet(String),
    #[error("module not found: {0}")]
    ModuleNotFound(String),
}

pub type CatalogResult<T> = Result<T, CatalogError>;

pub fn load_release_set_from_repo(
    repo_root: &Path,
    relative: &str,
) -> CatalogResult<ReleaseSetInfo> {
    load_release_set(&repo_root.join(relative))
}

pub fn upgrade_check_json(release_set: &ReleaseSetInfo, modules: &[ModuleUpgradeSummary]) -> Value {
    json!({
        "release_set": release_set.release,
        "status": release_set.status,
        "modules": modules,
        "update_count": modules.iter().filter(|module| module.status == UpgradeStatus::UpdateAvailable).count(),
    })
}
