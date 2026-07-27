use std::path::PathBuf;

use serde::Serialize;

use crate::release_set::{ReleaseSetInfo, ReleaseSetRepository};
use crate::{CatalogError, CatalogResult};

#[derive(Debug, thiserror::Error)]
pub enum RemoteHeadError {
    #[error("remote head lookup failed: {0}")]
    Lookup(String),
    #[error("unsupported repository url: {0}")]
    UnsupportedUrl(String),
}

pub trait RemoteHeadProvider: Send + Sync {
    fn resolve_head(&self, url: &str) -> Result<String, RemoteHeadError>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeStatus {
    UpToDate,
    UpdateAvailable,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModuleUpgradeSummary {
    pub id: String,
    pub url: String,
    pub kind: String,
    pub pinned_revision: String,
    pub remote_revision: Option<String>,
    pub status: UpgradeStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UpgradeStep {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub cli_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UpgradePlan {
    pub module_id: String,
    pub release_set: String,
    pub pinned_revision: String,
    pub target_revision: String,
    pub steps: Vec<UpgradeStep>,
    pub reload_hint: Option<String>,
}

pub fn check_module_updates(
    release_set: &ReleaseSetInfo,
    remote: &dyn RemoteHeadProvider,
) -> CatalogResult<Vec<ModuleUpgradeSummary>> {
    release_set
        .repositories
        .iter()
        .map(|repo| summarize_repo(repo, remote))
        .collect()
}

fn summarize_repo(
    repo: &ReleaseSetRepository,
    remote: &dyn RemoteHeadProvider,
) -> CatalogResult<ModuleUpgradeSummary> {
    let remote_revision = remote.resolve_head(&repo.url).ok();
    let status = match remote_revision.as_deref() {
        None => UpgradeStatus::Unknown,
        Some(head) if head.starts_with(&repo.revision) || repo.revision.starts_with(head) => {
            UpgradeStatus::UpToDate
        }
        Some(_) => UpgradeStatus::UpdateAvailable,
    };
    Ok(ModuleUpgradeSummary {
        id: repo.id.clone(),
        url: repo.url.clone(),
        kind: repo.kind.clone(),
        pinned_revision: repo.revision.clone(),
        remote_revision,
        status,
    })
}

pub fn plan_module_upgrade(
    release_set: &ReleaseSetInfo,
    module_id: &str,
    target_revision: Option<&str>,
) -> CatalogResult<UpgradePlan> {
    let repo = release_set
        .repositories
        .iter()
        .find(|repo| repo.id == module_id)
        .ok_or_else(|| CatalogError::ModuleNotFound(module_id.into()))?;
    let target = target_revision
        .map(str::to_string)
        .unwrap_or_else(|| repo.revision.clone());
    let steps = plan_steps(repo, &release_set.release, &target);
    let reload_hint = None;
    Ok(UpgradePlan {
        module_id: repo.id.clone(),
        release_set: release_set.release.clone(),
        pinned_revision: repo.revision.clone(),
        target_revision: target,
        steps,
        reload_hint,
    })
}

fn plan_steps(
    repo: &ReleaseSetRepository,
    release_set: &str,
    target_revision: &str,
) -> Vec<UpgradeStep> {
    let workspace_hint = sibling_checkout_hint(&repo.id);
    let mut steps = vec![
        UpgradeStep {
            id: "check".into(),
            title: "检查模块升级".into(),
            detail: format!(
                "对照 release `{release_set}` 中 `{id}` 的 pin `{pinned}` 与 Mutsuki 远端 `{target}`",
                id = repo.id,
                pinned = repo.revision,
                target = target_revision,
            ),
            cli_hint: Some("mutsuki-plugin check --release-set <product-release.toml>".to_string()),
        },
        UpgradeStep {
            id: "fetch".into(),
            title: "Git 获取目标 revision".into(),
            detail: format!(
                "在 `{workspace}` 执行 `git fetch origin && git checkout {target}`",
                workspace = workspace_hint,
                target = target_revision,
            ),
            cli_hint: Some(format!(
                "git -C {workspace} fetch origin && git -C {workspace} checkout {target_revision}",
                workspace = workspace_hint,
            )),
        },
    ];
    steps.push(UpgradeStep {
        id: "build".into(),
        title: "验证统一仓库".into(),
        detail: format!(
            "在 `{workspace}` 运行锁定的 Mutsuki Workspace 构建与测试；失败必须结构化退出",
            workspace = workspace_hint,
        ),
        cli_hint: Some(format!(
            "cargo test --workspace --all-targets --locked --manifest-path {workspace}/Cargo.toml",
            workspace = workspace_hint,
        )),
    });
    steps.push(UpgradeStep {
        id: "pin".into(),
        title: "更新统一 Git pin".into(),
        detail: format!(
            "把产品中所有 Mutsuki package 同时更新到 `{target}`，重新生成 lockfile 并运行 `cargo metadata --locked`",
            target = target_revision,
        ),
        cli_hint: Some("cargo update && cargo metadata --locked".into()),
    });
    steps.push(UpgradeStep {
        id: "reload".into(),
        title: "重载或重启".into(),
        detail:
            "统一 Mutsuki revision 更新后重启 ServiceRuntime，并验证 health、配置装配和插件库存"
                .into(),
        cli_hint: None,
    });
    steps
}

pub(crate) fn sibling_checkout_hint(module_id: &str) -> String {
    match module_id {
        "mutsuki" => "../Mutsuki".into(),
        other => format!("../{other}"),
    }
}

pub fn default_abi_inventory_dir(plugin_id: &str) -> PathBuf {
    PathBuf::from("plugins").join("installed").join(plugin_id)
}

#[derive(Clone, Default)]
pub struct FixtureRemoteHeadProvider {
    heads: std::collections::BTreeMap<String, String>,
}

impl FixtureRemoteHeadProvider {
    pub fn with_head(mut self, url: impl Into<String>, revision: impl Into<String>) -> Self {
        self.heads.insert(url.into(), revision.into());
        self
    }
}

impl RemoteHeadProvider for FixtureRemoteHeadProvider {
    fn resolve_head(&self, url: &str) -> Result<String, RemoteHeadError> {
        self.heads
            .get(url)
            .cloned()
            .ok_or_else(|| RemoteHeadError::Lookup(format!("no fixture head for {url}")))
    }
}

#[derive(Default)]
pub struct ReqwestRemoteHeadProvider {
    token: Option<String>,
}

impl RemoteHeadProvider for ReqwestRemoteHeadProvider {
    fn resolve_head(&self, url: &str) -> Result<String, RemoteHeadError> {
        let (owner, repo) = parse_github_url(url)?;
        let client = reqwest::blocking::Client::builder()
            .user_agent("mutsuki-plugin-catalog/0.1")
            .build()
            .map_err(|err| RemoteHeadError::Lookup(err.to_string()))?;
        let api_url = format!("https://api.github.com/repos/{owner}/{repo}/commits/HEAD");
        let mut request = client.get(api_url);
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = request
            .send()
            .map_err(|err| RemoteHeadError::Lookup(err.to_string()))?;
        if !response.status().is_success() {
            return Err(RemoteHeadError::Lookup(format!(
                "github status {} for {owner}/{repo}",
                response.status()
            )));
        }
        #[derive(serde::Deserialize)]
        struct CommitResponse {
            sha: String,
        }
        let body: CommitResponse = response
            .json()
            .map_err(|err| RemoteHeadError::Lookup(err.to_string()))?;
        Ok(body.sha)
    }
}

fn parse_github_url(url: &str) -> Result<(String, String), RemoteHeadError> {
    let trimmed = url.trim_end_matches('/').trim_end_matches(".git");
    let path = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))
        .ok_or_else(|| RemoteHeadError::UnsupportedUrl(url.into()))?;
    let mut parts = path.split('/');
    let owner = parts
        .next()
        .ok_or_else(|| RemoteHeadError::UnsupportedUrl(url.into()))?
        .to_string();
    let repo = parts
        .next()
        .ok_or_else(|| RemoteHeadError::UnsupportedUrl(url.into()))?
        .to_string();
    Ok((owner, repo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release_set::ReleaseSetRepository;

    fn sample_repo() -> ReleaseSetRepository {
        ReleaseSetRepository {
            id: "mutsuki".into(),
            url: "https://github.com/sena-nana/Mutsuki.git".into(),
            revision: "aaaa1111".into(),
            kind: "rust".into(),
        }
    }

    #[test]
    fn detects_update_available() {
        let remote = FixtureRemoteHeadProvider::default().with_head(
            "https://github.com/sena-nana/Mutsuki.git",
            "bbbb2222cccc3333",
        );
        let summary = summarize_repo(&sample_repo(), &remote).unwrap();
        assert_eq!(summary.status, UpgradeStatus::UpdateAvailable);
        assert_eq!(summary.remote_revision.as_deref(), Some("bbbb2222cccc3333"));
    }

    #[test]
    fn detects_up_to_date_prefix_match() {
        let remote = FixtureRemoteHeadProvider::default().with_head(
            "https://github.com/sena-nana/Mutsuki.git",
            "aaaa1111deadbeef",
        );
        let summary = summarize_repo(&sample_repo(), &remote).unwrap();
        assert_eq!(summary.status, UpgradeStatus::UpToDate);
    }

    #[test]
    fn plan_includes_fetch_build_pin_reload() {
        let release_set = ReleaseSetInfo {
            schema_version: 1,
            release: "v0.1.0".into(),
            status: "active".into(),
            contracts_api: "0.1.0".into(),
            runtime_wire_schema: "mutsuki.runtime.wire/1.3.0".into(),
            supported_deployments: vec![],
            unsupported_deployments: vec![],
            repositories: vec![sample_repo()],
        };
        let plan = plan_module_upgrade(&release_set, "mutsuki", Some("bbbb2222")).unwrap();
        assert_eq!(plan.target_revision, "bbbb2222");
        assert!(plan.steps.iter().any(|step| step.id == "fetch"));
        assert!(plan.steps.iter().any(|step| step.id == "build"));
        assert!(plan.steps.iter().any(|step| step.id == "pin"));
    }
}
