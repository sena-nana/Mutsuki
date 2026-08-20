use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_REPOSITORY: &str = "https://github.com/sena-nana/Mutsuki.git";
pub const DEFAULT_REVISION: &str = env!("MUTSUKI_CREATE_BOT_DEFAULT_REVISION");

#[must_use]
pub fn source_is_dirty() -> bool {
    env!("MUTSUKI_CREATE_BOT_SOURCE_DIRTY") == "true"
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateBotOptions {
    pub name: String,
    pub output: PathBuf,
    pub revision: String,
}

impl CreateBotOptions {
    #[must_use]
    pub fn new(name: impl Into<String>, output: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            output: output.into(),
            revision: DEFAULT_REVISION.into(),
        }
    }

    #[must_use]
    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = revision.into();
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedBotProject {
    pub root: PathBuf,
    pub revision: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateBotError {
    #[error(
        "invalid Bot project name `{0}`; use lowercase ASCII letters, digits, `-` or `_`, starting with a letter"
    )]
    InvalidName(String),
    #[error("invalid Mutsuki revision `{0}`; expected a full 40-character Git commit")]
    InvalidRevision(String),
    #[error("target already exists: {0}")]
    TargetExists(PathBuf),
    #[error("failed to create Bot project at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn create_bot_project(options: &CreateBotOptions) -> Result<CreatedBotProject, CreateBotError> {
    validate_project_name(&options.name)?;
    validate_revision(&options.revision)?;
    if options.output.exists() {
        return Err(CreateBotError::TargetExists(options.output.clone()));
    }

    let parent = options
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    let staging = tempfile::Builder::new()
        .prefix(".create-bot-")
        .tempdir_in(parent)
        .map_err(|source| io_error(parent, source))?;
    write_project(staging.path(), options)?;
    fs::rename(staging.path(), &options.output)
        .map_err(|source| io_error(&options.output, source))?;

    Ok(CreatedBotProject {
        root: options.output.clone(),
        revision: options.revision.clone(),
    })
}

fn write_project(root: &Path, options: &CreateBotOptions) -> Result<(), CreateBotError> {
    let source = root.join("src");
    fs::create_dir(&source).map_err(|error| io_error(&source, error))?;
    write(
        &root.join("Cargo.toml"),
        &cargo_manifest(&options.name, &options.revision),
    )?;
    write(&source.join("main.rs"), MAIN_RS)?;
    write(&root.join("README.md"), &readme(options))?;
    write(&root.join(".gitignore"), GITIGNORE)?;
    Ok(())
}

fn write(path: &Path, content: &str) -> Result<(), CreateBotError> {
    fs::write(path, content).map_err(|source| io_error(path, source))
}

fn cargo_manifest(name: &str, revision: &str) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
rust-version = "1.91"
license = "MIT"

[dependencies]
mutsuki-bot = {{ git = "{DEFAULT_REPOSITORY}", rev = "{revision}" }}
tokio = {{ version = "1.52", features = ["macros", "rt-multi-thread"] }}
"#
    )
}

fn readme(options: &CreateBotOptions) -> String {
    format!(
        r#"# {name}

This Bot is a thin product shell backed by Mutsuki revision `{revision}`. Runtime, Host, Agent,
Bot and platform behavior remain in their Mutsuki owner packages.

## Run

```bash
cargo run
```

The first run creates `Cargo.lock`; commit that lockfile and use `cargo run --locked` afterward.

On the first run, set and confirm the Console passphrase, open `http://127.0.0.1:8787`, then use
the same passphrase to sign in. Save QQ login, the model and reply policy under **配置**. Watch
connections and sessions under **Bot**. Save the message flow under **流程编排**; saving puts it
online. The minimal reply flow is:

```text
QQ 事件 -> 提交 Agent -> 可靠回复投递
```

The runtime creates `.mutsuki-bot/` beside the executable. Do not commit that directory or any
credential. For non-interactive startup, set `MUTSUKI_SECRET_MUTSUKI_WEB_CONSOLE_TOKEN`.
"#,
        name = options.name,
        revision = options.revision,
    )
}

pub fn validate_project_name(name: &str) -> Result<(), CreateBotError> {
    let mut bytes = name.bytes();
    let valid = bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte)
        });
    if valid {
        Ok(())
    } else {
        Err(CreateBotError::InvalidName(name.into()))
    }
}

pub fn validate_revision(revision: &str) -> Result<(), CreateBotError> {
    if revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(CreateBotError::InvalidRevision(revision.into()))
    }
}

fn io_error(path: &Path, source: std::io::Error) -> CreateBotError {
    CreateBotError::Io {
        path: path.to_path_buf(),
        source,
    }
}

const MAIN_RS: &str = r#"
#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    mutsuki_bot::run_single_instance_product_entry().await?;
    Ok(())
}
"#;

const GITIGNORE: &str = "/target\n.mutsuki-bot/\n";

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn generated_project_has_valid_cargo_metadata() {
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("hello-bot");
        let created = create_bot_project(&CreateBotOptions::new("hello-bot", &target)).unwrap();

        let manifest: toml::Value =
            toml::from_str(&fs::read_to_string(target.join("Cargo.toml")).unwrap()).unwrap();
        assert_eq!(manifest["package"]["name"].as_str(), Some("hello-bot"));
        assert_eq!(
            manifest["dependencies"]["mutsuki-bot"]["rev"].as_str(),
            Some(created.revision.as_str())
        );
        assert_eq!(
            manifest["dependencies"]["mutsuki-bot"]["git"].as_str(),
            Some(DEFAULT_REPOSITORY)
        );
        assert!(target.join("src/main.rs").is_file());

        let status = Command::new(env!("CARGO"))
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .current_dir(&target)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn existing_target_is_preserved() {
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("existing-bot");
        fs::create_dir(&target).unwrap();
        let sentinel = target.join("keep.txt");
        fs::write(&sentinel, "owned by user").unwrap();

        assert!(matches!(
            create_bot_project(&CreateBotOptions::new("existing-bot", &target)),
            Err(CreateBotError::TargetExists(path)) if path == target
        ));
        assert_eq!(fs::read_to_string(sentinel).unwrap(), "owned by user");
    }

    #[test]
    fn invalid_identity_is_rejected_before_writing() {
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("Bot Name");
        assert!(matches!(
            create_bot_project(&CreateBotOptions::new("Bot Name", &target)),
            Err(CreateBotError::InvalidName(_))
        ));
        assert!(!target.exists());

        let target = parent.path().join("valid-bot");
        assert!(matches!(
            create_bot_project(&CreateBotOptions::new("valid-bot", &target).with_revision("main")),
            Err(CreateBotError::InvalidRevision(_))
        ));
        assert!(!target.exists());
    }
}
