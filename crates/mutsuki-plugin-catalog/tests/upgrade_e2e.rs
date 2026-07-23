//! Release set module upgrade checks and planning.

use std::path::PathBuf;

use mutsuki_plugin_catalog::{
    FixtureRemoteHeadProvider, UpgradeStatus, check_module_updates, load_release_set,
    plan_module_upgrade,
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
fn plan_core_module_includes_build_and_pin_steps() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let release_set = load_release_set(&root.join("tests/fixtures/release-set.toml")).unwrap();
    let plan = plan_module_upgrade(&release_set, "core", Some("bbbb2222")).unwrap();
    assert_eq!(plan.target_revision, "bbbb2222");
    let step_ids: Vec<_> = plan.steps.iter().map(|step| step.id.as_str()).collect();
    assert!(step_ids.contains(&"fetch"));
    assert!(step_ids.contains(&"build"));
    assert!(step_ids.contains(&"pin"));
    assert!(step_ids.contains(&"reload"));
}
