//! Release set module upgrade checks and planning.

use std::path::PathBuf;

use mutsuki_plugin_catalog::{
    FixtureRemoteHeadProvider, UpgradeExecuteOptions, UpgradeStatus, check_module_updates,
    execute_module_upgrade, load_release_set,
};

#[test]
fn check_fixture_remote_reports_updates() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let release_set = load_release_set(&root.join("tests/fixtures/release-set.toml")).unwrap();
    let remote = FixtureRemoteHeadProvider::default()
        .with_head(
            "https://github.com/sena-nana/MutsukiCore.git",
            "bbbb2222cccc3333dddd4444",
        )
        .with_head(
            "https://github.com/sena-nana/MutsukiBotPlugins.git",
            "dddd4444eeee5555ffff6666",
        );
    let modules = check_module_updates(&release_set, &remote).unwrap();
    assert_eq!(modules.len(), 2);
    assert!(
        modules
            .iter()
            .all(|module| module.status == UpgradeStatus::UpdateAvailable)
    );
}

#[test]
fn execute_dry_run_emits_structured_report() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let release_set = load_release_set(&root.join("tests/fixtures/release-set.toml")).unwrap();
    let report = execute_module_upgrade(
        &release_set,
        &root.join("tests/fixtures/release-set.toml"),
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
    assert!(report.steps.iter().any(|step| step.id == "pin"));
}
