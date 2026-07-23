use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::release_set::ReleaseSetInfo;
use crate::upgrade::{
    UpgradePlan, default_abi_inventory_dir, plan_module_upgrade, sibling_checkout_hint,
};
use crate::{CatalogError, CatalogResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Skipped,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpgradeStepResult {
    pub id: String,
    pub status: StepStatus,
    pub detail: String,
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpgradeExecuteReport {
    pub module_id: String,
    pub target_revision: String,
    pub dry_run: bool,
    pub success: bool,
    pub steps: Vec<UpgradeStepResult>,
    pub pin_guidance: Option<String>,
    pub reload_hint: Option<String>,
    pub cli_command: String,
}

#[derive(Debug, Clone, Default)]
pub struct UpgradeExecuteOptions {
    pub dry_run: bool,
    pub workspace_root: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub skip_fetch: bool,
    pub skip_build: bool,
    pub skip_abi: bool,
    pub skip_pin: bool,
}

pub fn format_execute_cli_command(
    release_set_path: &Path,
    module_id: &str,
    target_revision: Option<&str>,
    options: &UpgradeExecuteOptions,
) -> String {
    let mut parts = vec![
        "mutsuki-plugin execute".into(),
        format!("--release-set {}", shell_quote(release_set_path)),
        format!("--module {}", shell_quote(module_id)),
    ];
    if let Some(rev) = target_revision {
        parts.push(format!("--target-rev {}", shell_quote(rev)));
    }
    if options.dry_run {
        parts.push("--dry-run".into());
    }
    if options.skip_fetch {
        parts.push("--skip-fetch".into());
    }
    if options.skip_build {
        parts.push("--skip-build".into());
    }
    if options.skip_abi {
        parts.push("--skip-abi".into());
    }
    if let Some(root) = &options.workspace_root {
        parts.push(format!("--workspace-root {}", shell_quote(root)));
    }
    if let Some(workspace) = &options.workspace {
        parts.push(format!("--workspace {}", shell_quote(workspace)));
    }
    parts.join(" ")
}

pub fn execute_module_upgrade(
    release_set: &ReleaseSetInfo,
    release_set_path: &Path,
    module_id: &str,
    target_revision: Option<&str>,
    options: &UpgradeExecuteOptions,
) -> CatalogResult<UpgradeExecuteReport> {
    let plan = plan_module_upgrade(release_set, module_id, target_revision)?;
    let workspace = resolve_workspace(&plan.module_id, options)?;
    let mut steps = Vec::new();
    let mut success = true;

    if !options.skip_fetch {
        let step = run_fetch_step(&workspace, &plan, options.dry_run)?;
        success &= step.status != StepStatus::Failed;
        steps.push(step);
    } else {
        steps.push(skipped_step("fetch", "已跳过 Git 获取"));
    }

    if !matches_repo_kind(release_set, &plan.module_id, "python") {
        if !options.skip_build {
            let step = run_build_step(&workspace, options.dry_run)?;
            success &= step.status != StepStatus::Failed;
            steps.push(step);
        } else {
            steps.push(skipped_step("build", "已跳过编译"));
        }
    }

    if should_run_abi(release_set, &plan.module_id) && !options.skip_abi {
        let step = run_abi_step(&workspace, &plan.module_id, options.dry_run)?;
        success &= step.status != StepStatus::Failed;
        steps.push(step);
    } else if should_run_abi(release_set, &plan.module_id) {
        steps.push(skipped_step("abi", "已跳过 ABI 校验"));
    }

    let pin_guidance = if options.skip_pin {
        None
    } else {
        Some(pin_guidance_text(
            release_set,
            release_set_path,
            &plan,
            options,
        ))
    };
    if !options.skip_pin {
        steps.push(UpgradeStepResult {
            id: "pin".into(),
            status: if options.dry_run {
                StepStatus::Skipped
            } else {
                StepStatus::Succeeded
            },
            detail: pin_guidance.clone().unwrap_or_default(),
            command: Some(format!(
                "python3 scripts/release_set.py --manifest {} sync --workspace-root {}",
                shell_quote(release_set_path),
                shell_quote(options.workspace_root.as_deref().unwrap_or(Path::new("..")))
            )),
        });
    }

    steps.push(UpgradeStepResult {
        id: "reload".into(),
        status: StepStatus::Skipped,
        detail: plan
            .reload_hint
            .clone()
            .unwrap_or_else(|| "核心模块 pin 更新后重启 ServiceRuntime".into()),
        command: plan.reload_hint.clone(),
    });

    Ok(UpgradeExecuteReport {
        module_id: plan.module_id.clone(),
        target_revision: plan.target_revision.clone(),
        dry_run: options.dry_run,
        success,
        steps,
        pin_guidance,
        reload_hint: plan.reload_hint.clone(),
        cli_command: format_execute_cli_command(
            release_set_path,
            module_id,
            Some(plan.target_revision.as_str()),
            options,
        ),
    })
}

fn matches_repo_kind(release_set: &ReleaseSetInfo, module_id: &str, kind: &str) -> bool {
    release_set
        .repositories
        .iter()
        .find(|repo| repo.id == module_id)
        .is_some_and(|repo| repo.kind == kind)
}

fn should_run_abi(release_set: &ReleaseSetInfo, module_id: &str) -> bool {
    release_set
        .repositories
        .iter()
        .find(|repo| repo.id == module_id)
        .is_some_and(|repo| repo.id == "bot_plugins" || repo.id.ends_with("_plugins"))
}

fn resolve_workspace(module_id: &str, options: &UpgradeExecuteOptions) -> CatalogResult<PathBuf> {
    if let Some(workspace) = &options.workspace {
        return Ok(workspace.clone());
    }
    let root = options
        .workspace_root
        .clone()
        .unwrap_or_else(|| PathBuf::from(".."));
    let relative = sibling_checkout_hint(module_id);
    let workspace = root.join(relative.trim_start_matches("../"));
    if !options.dry_run && !workspace.is_dir() {
        return Err(CatalogError::ReleaseSet(format!(
            "workspace not found: {} (pass --workspace or --workspace-root)",
            workspace.display()
        )));
    }
    Ok(workspace)
}

fn run_fetch_step(
    workspace: &Path,
    plan: &UpgradePlan,
    dry_run: bool,
) -> CatalogResult<UpgradeStepResult> {
    let fetch_cmd = format!(
        "git -C {} fetch origin && git -C {} checkout {}",
        shell_quote(workspace),
        shell_quote(workspace),
        shell_quote(&plan.target_revision)
    );
    if dry_run {
        return Ok(UpgradeStepResult {
            id: "fetch".into(),
            status: StepStatus::Skipped,
            detail: format!(
                "将获取并 checkout `{target}` 到 `{workspace}`",
                target = plan.target_revision,
                workspace = workspace.display()
            ),
            command: Some(fetch_cmd),
        });
    }
    if !workspace.join(".git").exists() {
        return Ok(failed_step(
            "fetch",
            format!("{} 不是 Git 仓库", workspace.display()),
            Some(fetch_cmd),
        ));
    }
    let fetch = run_command(
        Command::new("git")
            .arg("-C")
            .arg(workspace)
            .arg("fetch")
            .arg("origin"),
    );
    if !fetch.success {
        return Ok(failed_step(
            "fetch",
            format_command_failure("git fetch origin", &fetch),
            Some(fetch_cmd.clone()),
        ));
    }
    let checkout = run_command(
        Command::new("git")
            .arg("-C")
            .arg(workspace)
            .arg("checkout")
            .arg(&plan.target_revision),
    );
    if checkout.success {
        Ok(UpgradeStepResult {
            id: "fetch".into(),
            status: StepStatus::Succeeded,
            detail: format!("已 checkout `{}`", plan.target_revision),
            command: Some(fetch_cmd),
        })
    } else {
        Ok(failed_step(
            "fetch",
            format_command_failure("git checkout", &checkout),
            Some(fetch_cmd),
        ))
    }
}

fn run_build_step(workspace: &Path, dry_run: bool) -> CatalogResult<UpgradeStepResult> {
    let manifest = workspace.join("Cargo.toml");
    let build_cmd = format!(
        "cargo build --release --manifest-path {}",
        shell_quote(&manifest)
    );
    if dry_run {
        return Ok(UpgradeStepResult {
            id: "build".into(),
            status: StepStatus::Skipped,
            detail: format!("将在 `{}` 编译 release", workspace.display()),
            command: Some(build_cmd),
        });
    }
    if !manifest.is_file() {
        return Ok(failed_step(
            "build",
            format!("缺少 Cargo.toml: {}", manifest.display()),
            Some(build_cmd),
        ));
    }
    let output = run_command(
        Command::new("cargo")
            .arg("build")
            .arg("--release")
            .arg("--manifest-path")
            .arg(&manifest),
    );
    if output.success {
        Ok(UpgradeStepResult {
            id: "build".into(),
            status: StepStatus::Succeeded,
            detail: "release 编译成功".into(),
            command: Some(build_cmd),
        })
    } else {
        Ok(failed_step(
            "build",
            format_command_failure("cargo build --release", &output),
            Some(build_cmd),
        ))
    }
}

fn run_abi_step(
    workspace: &Path,
    module_id: &str,
    dry_run: bool,
) -> CatalogResult<UpgradeStepResult> {
    let target_dir = workspace.join("target").join("release");
    let inventory = default_abi_inventory_dir(module_id);
    let detail = format!(
        "校验 `{target}` 中的动态库并将产物指引写入 `{inventory}`",
        target = target_dir.display(),
        inventory = inventory.display()
    );
    if dry_run {
        return Ok(UpgradeStepResult {
            id: "abi".into(),
            status: StepStatus::Skipped,
            detail,
            command: None,
        });
    }
    let artifacts = collect_dynamic_libraries(&target_dir);
    if artifacts.is_empty() {
        return Ok(failed_step(
            "abi",
            format!("release 目录未找到动态库: {}", target_dir.display()),
            None,
        ));
    }
    let mut summaries = Vec::new();
    for artifact in artifacts {
        let hash = sha256_file(&artifact)?;
        summaries.push(format!(
            "{} sha256:{}",
            artifact
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("artifact"),
            hash
        ));
    }
    Ok(UpgradeStepResult {
        id: "abi".into(),
        status: StepStatus::Succeeded,
        detail: format!(
            "{detail}; 发现 {} 个 artifact: {}",
            summaries.len(),
            summaries.join(", ")
        ),
        command: None,
    })
}

fn collect_dynamic_libraries(dir: &Path) -> Vec<PathBuf> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read_dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(ext, "so" | "dylib" | "dll"))
        })
        .collect()
}

fn sha256_file(path: &Path) -> CatalogResult<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|source| CatalogError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn pin_guidance_text(
    release_set: &ReleaseSetInfo,
    release_set_path: &Path,
    plan: &UpgradePlan,
    options: &UpgradeExecuteOptions,
) -> String {
    let repo = release_set
        .repositories
        .iter()
        .find(|repo| repo.id == plan.module_id);
    let workspace_root = options.workspace_root.as_deref().unwrap_or(Path::new(".."));
    match repo {
        Some(repo) => format!(
            "更新 release set `{}` 中 `{}` 的 revision 为 `{}`，运行 release_set sync，并在产品仓库执行 `cargo metadata --locked`。manifest: {}",
            release_set.release,
            repo.id,
            plan.target_revision,
            release_set_path.display()
        ),
        None => format!(
            "更新 `{manifest}` 中 `{module}` pin 到 `{target}`，workspace-root `{root}`",
            manifest = release_set_path.display(),
            module = plan.module_id,
            target = plan.target_revision,
            root = workspace_root.display()
        ),
    }
}

struct CommandOutput {
    success: bool,
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

fn run_command(command: &mut Command) -> CommandOutput {
    match command.output() {
        Ok(output) => CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            code: output.status.code(),
        },
        Err(error) => CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: error.to_string(),
            code: None,
        },
    }
}

fn format_command_failure(label: &str, output: &CommandOutput) -> String {
    let mut detail = format!("{label} failed");
    if let Some(code) = output.code {
        detail.push_str(&format!(" (exit {code})"));
    }
    if !output.stderr.is_empty() {
        detail.push_str(": ");
        detail.push_str(&output.stderr);
    } else if !output.stdout.is_empty() {
        detail.push_str(": ");
        detail.push_str(&output.stdout);
    }
    detail
}

fn skipped_step(id: &str, detail: impl Into<String>) -> UpgradeStepResult {
    UpgradeStepResult {
        id: id.into(),
        status: StepStatus::Skipped,
        detail: detail.into(),
        command: None,
    }
}

fn failed_step(id: &str, detail: impl Into<String>, command: Option<String>) -> UpgradeStepResult {
    UpgradeStepResult {
        id: id.into(),
        status: StepStatus::Failed,
        detail: detail.into(),
        command,
    }
}

fn shell_quote(value: impl AsRef<Path>) -> String {
    let text = value.as_ref().to_string_lossy();
    if text
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '-' | '_' | '.'))
    {
        text.into_owned()
    } else {
        format!("'{text}'")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release_set::ReleaseSetRepository;

    fn sample_release_set() -> ReleaseSetInfo {
        ReleaseSetInfo {
            schema_version: 1,
            release: "mutsuki-0.1-alpha-3".into(),
            status: "active".into(),
            contracts_api: "0.1.0".into(),
            runtime_wire_schema: "mutsuki.runtime.wire/1.3.0".into(),
            supported_deployments: vec![],
            unsupported_deployments: vec![],
            repositories: vec![ReleaseSetRepository {
                id: "core".into(),
                url: "https://github.com/sena-nana/MutsukiCore.git".into(),
                revision: "aaaa1111".into(),
                kind: "rust".into(),
            }],
        }
    }

    #[test]
    fn dry_run_reports_steps_without_touching_workspace() {
        let release_set = sample_release_set();
        let report = execute_module_upgrade(
            &release_set,
            Path::new("releases/mutsuki-0.1-alpha-3.toml"),
            "core",
            Some("bbbb2222"),
            &UpgradeExecuteOptions {
                dry_run: true,
                workspace: Some(PathBuf::from("/tmp/mutsuki-core")),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(report.success);
        assert!(report.dry_run);
        assert!(report.steps.iter().any(|step| step.id == "fetch"));
        assert!(report.steps.iter().any(|step| step.id == "build"));
        assert!(report.cli_command.contains("mutsuki-plugin execute"));
    }

    #[test]
    fn missing_git_repo_fails_fetch_step() {
        let release_set = sample_release_set();
        let dir = tempfile::tempdir().unwrap();
        let report = execute_module_upgrade(
            &release_set,
            Path::new("releases/mutsuki-0.1-alpha-3.toml"),
            "core",
            Some("bbbb2222"),
            &UpgradeExecuteOptions {
                dry_run: false,
                workspace: Some(dir.path().to_path_buf()),
                skip_build: true,
                skip_abi: true,
                skip_pin: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!report.success);
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.id == "fetch" && step.status == StepStatus::Failed)
        );
    }
}
