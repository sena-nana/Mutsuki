use std::path::PathBuf;
use std::process;

use mutsuki_plugin_catalog::{
    FixtureRemoteHeadProvider, UpgradeExecuteOptions, check_module_updates, execute_module_upgrade,
    load_release_set, plan_module_upgrade, upgrade_check_json,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("mutsuki-plugin error: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        print_usage();
        return Ok(());
    };
    match command.as_str() {
        "check" => check_command(args.collect())?,
        "plan" => plan_command(args.collect())?,
        "execute" => execute_command(args.collect())?,
        "help" | "--help" | "-h" => print_usage(),
        other => return Err(format!("unknown command `{other}`").into()),
    }
    Ok(())
}

fn check_command(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut release_set_path = None;
    let mut fixture = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--release-set" => {
                release_set_path = Some(PathBuf::from(
                    iter.next().ok_or("--release-set requires path")?,
                ))
            }
            "--fixture-remote" => fixture = true,
            other => return Err(format!("unknown flag `{other}`").into()),
        }
    }
    let release_set_path = release_set_path.ok_or("--release-set is required")?;
    let release_set = load_release_set(&release_set_path)?;
    let remote: Box<dyn mutsuki_plugin_catalog::RemoteHeadProvider> = if fixture {
        build_fixture_remote(&release_set)
    } else {
        Box::new(mutsuki_plugin_catalog::ReqwestRemoteHeadProvider::default())
    };
    let modules = check_module_updates(&release_set, remote.as_ref())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&upgrade_check_json(&release_set, &modules))?
    );
    Ok(())
}

fn plan_command(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut release_set_path = None;
    let mut module_id = None;
    let mut target_revision = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--release-set" => {
                release_set_path = Some(PathBuf::from(
                    iter.next().ok_or("--release-set requires path")?,
                ))
            }
            "--module" => module_id = Some(iter.next().ok_or("--module requires id")?),
            "--target-rev" => {
                target_revision = Some(iter.next().ok_or("--target-rev requires value")?)
            }
            other => return Err(format!("unknown flag `{other}`").into()),
        }
    }
    let release_set_path = release_set_path.ok_or("--release-set is required")?;
    let module_id = module_id.ok_or("--module is required")?;
    let release_set = load_release_set(&release_set_path)?;
    let plan = plan_module_upgrade(&release_set, &module_id, target_revision.as_deref())?;
    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}

fn execute_command(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut release_set_path = None;
    let mut module_id = None;
    let mut target_revision = None;
    let mut options = UpgradeExecuteOptions::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--release-set" => {
                release_set_path = Some(PathBuf::from(
                    iter.next().ok_or("--release-set requires path")?,
                ))
            }
            "--module" => module_id = Some(iter.next().ok_or("--module requires id")?),
            "--target-rev" => {
                target_revision = Some(iter.next().ok_or("--target-rev requires value")?)
            }
            "--dry-run" => options.dry_run = true,
            "--skip-fetch" => options.skip_fetch = true,
            "--skip-build" => options.skip_build = true,
            "--skip-abi" => options.skip_abi = true,
            "--skip-pin" => options.skip_pin = true,
            "--workspace-root" => {
                options.workspace_root = Some(PathBuf::from(
                    iter.next().ok_or("--workspace-root requires path")?,
                ))
            }
            "--workspace" => {
                options.workspace = Some(PathBuf::from(
                    iter.next().ok_or("--workspace requires path")?,
                ))
            }
            other => return Err(format!("unknown flag `{other}`").into()),
        }
    }
    let release_set_path = release_set_path.ok_or("--release-set is required")?;
    let module_id = module_id.ok_or("--module is required")?;
    let release_set = load_release_set(&release_set_path)?;
    let report = execute_module_upgrade(
        &release_set,
        &release_set_path,
        &module_id,
        target_revision.as_deref(),
        &options,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !report.success {
        process::exit(2);
    }
    Ok(())
}

fn build_fixture_remote(
    release_set: &mutsuki_plugin_catalog::ReleaseSetInfo,
) -> Box<dyn mutsuki_plugin_catalog::RemoteHeadProvider> {
    let mut fixture = FixtureRemoteHeadProvider::default();
    for repo in &release_set.repositories {
        fixture = fixture.with_head(
            repo.url.clone(),
            "ffffffffffffffffffffffffffffffffffffffff".to_string(),
        );
    }
    Box::new(fixture)
}

fn print_usage() {
    eprintln!(
        "Usage:
  mutsuki-plugin check --release-set <manifest.toml> [--fixture-remote]
  mutsuki-plugin plan --release-set <manifest.toml> --module <id> [--target-rev <rev>]
  mutsuki-plugin execute --release-set <manifest.toml> --module <id> [--target-rev <rev>] [--dry-run]
      [--workspace-root <path>] [--workspace <path>] [--skip-fetch] [--skip-build] [--skip-abi] [--skip-pin]

Examples:
  mutsuki-plugin check --release-set releases/mutsuki-0.1-alpha-3.toml --fixture-remote
  mutsuki-plugin plan --release-set releases/mutsuki-0.1-alpha-3.toml --module core --target-rev <sha>
  mutsuki-plugin execute --release-set releases/mutsuki-0.1-alpha-3.toml --module core --dry-run --workspace ../MutsukiCore"
    );
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use mutsuki_plugin_catalog::UpgradeStatus;

    #[test]
    fn fixture_remote_marks_modules_upgradable() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let release_set = load_release_set(&root.join("tests/fixtures/release-set.toml")).unwrap();
        let remote = build_fixture_remote(&release_set);
        let modules = check_module_updates(&release_set, remote.as_ref()).unwrap();
        assert!(
            modules
                .iter()
                .any(|module| module.status == UpgradeStatus::UpdateAvailable)
        );
    }

    #[test]
    fn execute_dry_run_fixture_release_set() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let release_set_path = root.join("tests/fixtures/release-set.toml");
        let release_set = load_release_set(&release_set_path).unwrap();
        let report = execute_module_upgrade(
            &release_set,
            &release_set_path,
            "core",
            Some("bbbb2222"),
            &UpgradeExecuteOptions {
                dry_run: true,
                workspace: Some(root.join("..").join("..")),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(report.success);
        assert!(report.cli_command.contains("execute"));
    }
}
