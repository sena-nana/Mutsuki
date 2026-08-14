use std::process::Command;

const REVISION_ENV: &str = "MUTSUKI_CREATE_BOT_REVISION";

fn main() {
    println!("cargo:rerun-if-env-changed={REVISION_ENV}");
    track_repository_revision();

    let dirty = git_output(&["status", "--porcelain", "--untracked-files=normal"])
        .is_some_and(|status| !status.is_empty());
    println!("cargo:rustc-env=MUTSUKI_CREATE_BOT_SOURCE_DIRTY={dirty}");

    let revision = std::env::var(REVISION_ENV)
        .ok()
        .or_else(repository_revision)
        .filter(|revision| valid_revision(revision))
        .unwrap_or_else(|| {
            panic!(
                "create-bot requires a 40-character Mutsuki Git revision; set {REVISION_ENV} when building outside a Git checkout"
            )
        });
    println!("cargo:rustc-env=MUTSUKI_CREATE_BOT_DEFAULT_REVISION={revision}");
}

fn repository_revision() -> Option<String> {
    git_output(&["rev-parse", "HEAD"])
}

fn track_repository_revision() {
    for arguments in [
        &["rev-parse", "--git-path", "HEAD"][..],
        &["symbolic-ref", "-q", "HEAD"][..],
    ] {
        let Some(path_or_ref) = git_output(arguments) else {
            continue;
        };
        let path = if arguments[0] == "symbolic-ref" {
            let Some(path) = git_output(&["rev-parse", "--git-path", &path_or_ref]) else {
                continue;
            };
            path
        } else {
            path_or_ref
        };
        println!("cargo:rerun-if-changed={path}");
    }
}

fn git_output(arguments: &[&str]) -> Option<String> {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest_dir)
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
}

fn valid_revision(revision: &str) -> bool {
    revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
}
