use std::path::PathBuf;
use std::process;

use mutsuki_plugin_catalog::{
    FixtureRemoteHeadProvider, check_module_updates, load_release_set, plan_module_upgrade,
    upgrade_check_json,
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

Examples:
  mutsuki-plugin check --release-set releases/mutsuki-0.1-alpha-3.toml --fixture-remote
  mutsuki-plugin plan --release-set releases/mutsuki-0.1-alpha-3.toml --module core --target-rev <sha>"
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
}
