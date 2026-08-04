//! MomoBako 桌面插件包的发现、校验与内容寻址暂存。

use crate::config::MutsukiTauriConfig;
use crate::error::{HostError, HostResult};
use crate::plugin_path::{component_text, relative_path_string, safe_relative_path};
use mutsuki_runtime_contracts::{ArtifactType, PluginManifest};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// 已通过包结构校验的 companion artifact。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedCompanionArtifact {
    pub path: PathBuf,
    pub executable: bool,
    pub role: Option<String>,
}

/// `.momoplug` 的桌面 Host inventory 记录。
#[derive(Clone)]
pub struct PluginPackageRecord {
    pub archive_path: PathBuf,
    pub package_sha256: String,
    pub root_dir: String,
    pub plugin_id: String,
    pub version: String,
    pub selected: bool,
    pub executable: bool,
    pub runtime_manifest: Option<PluginManifest>,
    pub staged_artifact: Option<PathBuf>,
    pub staged_companions: Vec<StagedCompanionArtifact>,
    pub(crate) config: serde_json::Value,
    pub error: Option<String>,
}

impl PluginPackageRecord {
    pub(crate) fn invalid(archive_path: PathBuf, error: impl Into<String>) -> Self {
        let plugin_id = archive_path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(|value| format!("invalid:{value}"))
            .unwrap_or_else(|| "invalid:momoplug".into());
        Self {
            archive_path,
            package_sha256: String::new(),
            root_dir: String::new(),
            plugin_id,
            version: "unknown".into(),
            selected: false,
            executable: false,
            runtime_manifest: None,
            staged_artifact: None,
            staged_companions: Vec::new(),
            config: serde_json::Value::Null,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProductManifest {
    plugin_id: String,
    version: String,
    #[serde(default)]
    runtime: String,
    #[serde(default)]
    entry: serde_json::Value,
}

impl ProductManifest {
    fn has_executable_backend(&self) -> bool {
        self.entry
            .get("backend")
            .is_some_and(|entry| !entry.is_null())
            || matches!(
                self.runtime.as_str(),
                "abi" | "native-dylib" | "process" | "python"
            )
    }
}

#[derive(Debug)]
struct ArchiveLayout {
    root_dir: String,
    manifest_name: String,
    plugin_toml_name: Option<String>,
    files: BTreeSet<String>,
}

#[derive(Debug)]
struct DeclaredArtifact {
    archive_name: String,
    relative_path: PathBuf,
    expected_sha256: String,
    executable: bool,
    role: Option<String>,
    primary: bool,
}

pub(crate) fn scan_momoplug_packages(
    config: &MutsukiTauriConfig,
) -> HostResult<Vec<PluginPackageRecord>> {
    let archives = find_momoplug_archives(&config.paths.plugins_dir)?;
    Ok(archives
        .into_iter()
        .map(|path| {
            load_package(&path, config)
                .unwrap_or_else(|error| PluginPackageRecord::invalid(path, error))
        })
        .collect())
}

fn load_package(
    archive_path: &Path,
    config: &MutsukiTauriConfig,
) -> Result<PluginPackageRecord, String> {
    let package_sha256 = hash_file(archive_path)?;
    let file = File::open(archive_path)
        .map_err(|error| format!("failed to open {}: {error}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("failed to open {} as zip: {error}", archive_path.display()))?;
    let layout = validate_archive_layout(&mut archive)?;
    let product_text = read_small_text_entry(&mut archive, &layout.manifest_name)?;
    let product: ProductManifest = serde_json::from_str(&product_text)
        .map_err(|error| format!("failed to parse {}: {error}", layout.manifest_name))?;
    validate_identity("manifest.json", &product.plugin_id, &product.version)?;

    let selected = config.plugin_selection.is_enabled(&product.plugin_id);
    let plugin_config = config.plugin_selection.config_for(&product.plugin_id);
    let executable = product.has_executable_backend() || layout.plugin_toml_name.is_some();
    let Some(plugin_toml_name) = layout.plugin_toml_name.as_deref() else {
        if executable {
            return Err(format!(
                "executable plugin {} requires plugin.toml",
                product.plugin_id
            ));
        }
        return Ok(PluginPackageRecord {
            archive_path: archive_path.to_path_buf(),
            package_sha256,
            root_dir: layout.root_dir,
            plugin_id: product.plugin_id,
            version: product.version,
            selected,
            executable: false,
            runtime_manifest: None,
            staged_artifact: None,
            staged_companions: Vec::new(),
            config: plugin_config,
            error: None,
        });
    };

    let plugin_text = read_small_text_entry(&mut archive, plugin_toml_name)?;
    let plugin_value: toml::Value = toml::from_str(&plugin_text)
        .map_err(|error| format!("failed to parse {plugin_toml_name}: {error}"))?;
    let runtime_manifest: PluginManifest = plugin_value
        .clone()
        .try_into()
        .map_err(|error| format!("failed to parse {plugin_toml_name}: {error}"))?;
    validate_identity(
        "plugin.toml",
        &runtime_manifest.plugin_id,
        &runtime_manifest.version,
    )?;
    if runtime_manifest.plugin_id != product.plugin_id
        || runtime_manifest.version != product.version
    {
        return Err(format!(
            "manifest identity mismatch: manifest.json is {}@{}, plugin.toml is {}@{}",
            product.plugin_id,
            product.version,
            runtime_manifest.plugin_id,
            runtime_manifest.version
        ));
    }
    if !matches!(
        runtime_manifest.artifact.artifact_type,
        ArtifactType::Abi | ArtifactType::Process | ArtifactType::Python
    ) {
        return Err(format!(
            "desktop package {} uses unsupported executable artifact {:?}",
            product.plugin_id, runtime_manifest.artifact.artifact_type
        ));
    }

    let declared = declared_artifacts(&layout, &runtime_manifest)?;
    validate_declared_artifacts(&mut archive, &declared)?;
    let (staged_artifact, staged_companions) = if selected {
        stage_declared_artifacts(
            archive_path,
            &package_sha256,
            &config.paths.cache_dir,
            &declared,
        )?
    } else {
        (None, Vec::new())
    };

    Ok(PluginPackageRecord {
        archive_path: archive_path.to_path_buf(),
        package_sha256,
        root_dir: layout.root_dir,
        plugin_id: product.plugin_id,
        version: product.version,
        selected,
        executable: true,
        runtime_manifest: Some(runtime_manifest),
        staged_artifact,
        staged_companions,
        config: plugin_config,
        error: None,
    })
}

fn validate_identity(source: &str, plugin_id: &str, version: &str) -> Result<(), String> {
    if plugin_id.trim().is_empty() {
        return Err(format!("{source} is missing plugin id"));
    }
    if version.trim().is_empty() {
        return Err(format!("{source} is missing version"));
    }
    Ok(())
}

fn declared_artifacts(
    layout: &ArchiveLayout,
    manifest: &PluginManifest,
) -> Result<Vec<DeclaredArtifact>, String> {
    let mut declared = Vec::with_capacity(manifest.artifact.companion_artifacts.len() + 1);
    declared.push(declared_artifact(
        layout,
        &manifest.artifact.path,
        &manifest.artifact.sha256,
        true,
        Some("plugin".into()),
        true,
    )?);
    for companion in &manifest.artifact.companion_artifacts {
        declared.push(declared_artifact(
            layout,
            &companion.path,
            &companion.sha256,
            companion.executable,
            companion.role.clone(),
            false,
        )?);
    }
    let mut paths = BTreeSet::new();
    for artifact in &declared {
        if !paths.insert(artifact.relative_path.clone()) {
            return Err(format!(
                "duplicate declared artifact path: {}",
                artifact.relative_path.display()
            ));
        }
    }
    Ok(declared)
}

fn declared_artifact(
    layout: &ArchiveLayout,
    path: &str,
    sha256: &str,
    executable: bool,
    role: Option<String>,
    primary: bool,
) -> Result<DeclaredArtifact, String> {
    let relative_path = safe_relative_path(path)?;
    let normalized = relative_path_string(&relative_path);
    let archive_name = format!("{}/{normalized}", layout.root_dir);
    if !layout.files.contains(&archive_name) {
        return Err(format!("declared artifact is missing from archive: {path}"));
    }
    Ok(DeclaredArtifact {
        archive_name,
        relative_path,
        expected_sha256: normalize_sha256(sha256)?,
        executable,
        role,
        primary,
    })
}

fn validate_declared_artifacts<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    declared: &[DeclaredArtifact],
) -> Result<(), String> {
    for artifact in declared {
        let entry = archive
            .by_name(&artifact.archive_name)
            .map_err(|error| format!("failed to read {}: {error}", artifact.archive_name))?;
        let actual = hash_reader(entry)?;
        if actual != artifact.expected_sha256 {
            return Err(format!(
                "sha256 mismatch for {}: expected {}, got {}",
                artifact.relative_path.display(),
                artifact.expected_sha256,
                actual
            ));
        }
    }
    Ok(())
}

fn stage_declared_artifacts(
    archive_path: &Path,
    package_sha256: &str,
    cache_dir: &Path,
    declared: &[DeclaredArtifact],
) -> Result<(Option<PathBuf>, Vec<StagedCompanionArtifact>), String> {
    let runtime_root = cache_dir.join("plugin-runtime");
    let target_dir = runtime_root.join(package_sha256);
    if target_dir.is_dir() {
        validate_staged_artifacts(&target_dir, declared)?;
    } else {
        fs::create_dir_all(&runtime_root)
            .map_err(|error| format!("failed to create {}: {error}", runtime_root.display()))?;
        let temporary_dir =
            runtime_root.join(format!(".stage-{package_sha256}-{}", Uuid::new_v4()));
        fs::create_dir(&temporary_dir)
            .map_err(|error| format!("failed to create {}: {error}", temporary_dir.display()))?;
        let result =
            extract_declared_artifacts(archive_path, &temporary_dir, declared).and_then(|_| {
                fs::rename(&temporary_dir, &target_dir).map_err(|error| {
                    format!(
                        "failed to activate plugin cache {}: {error}",
                        target_dir.display()
                    )
                })
            });
        if result.is_err() {
            let _ = fs::remove_dir_all(&temporary_dir);
        }
        result?;
    }

    let primary = declared
        .iter()
        .find(|artifact| artifact.primary)
        .map(|artifact| target_dir.join(&artifact.relative_path));
    let companions = declared
        .iter()
        .filter(|artifact| !artifact.primary)
        .map(|artifact| StagedCompanionArtifact {
            path: target_dir.join(&artifact.relative_path),
            executable: artifact.executable,
            role: artifact.role.clone(),
        })
        .collect();
    Ok((primary, companions))
}

fn validate_staged_artifacts(
    target_dir: &Path,
    declared: &[DeclaredArtifact],
) -> Result<(), String> {
    for artifact in declared {
        let target = target_dir.join(&artifact.relative_path);
        let metadata = fs::symlink_metadata(&target)
            .map_err(|error| format!("failed to inspect cached {}: {error}", target.display()))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(format!(
                "cached plugin artifact is not a regular file: {}",
                target.display()
            ));
        }
        let actual = hash_file(&target)?;
        if actual != artifact.expected_sha256 {
            return Err(format!(
                "cached plugin artifact hash mismatch for {}",
                target.display()
            ));
        }
    }
    Ok(())
}

fn extract_declared_artifacts(
    archive_path: &Path,
    target_dir: &Path,
    declared: &[DeclaredArtifact],
) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|error| format!("failed to open {}: {error}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("failed to open {} as zip: {error}", archive_path.display()))?;
    for artifact in declared {
        let mut entry = archive
            .by_name(&artifact.archive_name)
            .map_err(|error| format!("failed to read {}: {error}", artifact.archive_name))?;
        let target = target_dir.join(&artifact.relative_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        let mut output = File::create(&target)
            .map_err(|error| format!("failed to create {}: {error}", target.display()))?;
        std::io::copy(&mut entry, &mut output)
            .map_err(|error| format!("failed to extract {}: {error}", target.display()))?;
        set_executable_if_requested(&target, artifact.executable)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable_if_requested(path: &Path, executable: bool) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if executable {
        let mut permissions = fs::metadata(path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        fs::set_permissions(path, permissions)
            .map_err(|error| format!("failed to chmod {}: {error}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_executable_if_requested(_path: &Path, _executable: bool) -> Result<(), String> {
    Ok(())
}

fn validate_archive_layout<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<ArchiveLayout, String> {
    let mut names = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut roots = BTreeSet::new();
    let mut manifest_names = Vec::new();
    let mut plugin_toml_names = Vec::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("failed to inspect zip entry {index}: {error}"))?;
        let raw_name = entry.name();
        let relative = safe_relative_path(raw_name)?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!(
                "plugin archive must not contain symlinks: {raw_name}"
            ));
        }
        let normalized = relative_path_string(&relative);
        if !names.insert(normalized.clone()) {
            return Err(format!(
                "plugin archive contains duplicate path: {normalized}"
            ));
        }
        if !entry.is_dir() {
            files.insert(normalized.clone());
        }
        let mut components = relative.components();
        let root = components
            .next()
            .and_then(component_text)
            .ok_or_else(|| format!("unsafe plugin archive path: {raw_name}"))?;
        roots.insert(root.to_string());
        let tail = components
            .map(|component| component_text(component).unwrap_or_default())
            .collect::<Vec<_>>();
        if tail.as_slice() == ["manifest.json"] {
            ensure_small_manifest(&entry, &normalized)?;
            manifest_names.push(normalized.clone());
        } else if tail.as_slice() == ["plugin.toml"] {
            ensure_small_manifest(&entry, &normalized)?;
            plugin_toml_names.push(normalized);
        }
    }
    if roots.len() != 1 || manifest_names.len() != 1 {
        return Err(
            "plugin archive must contain exactly one root directory with manifest.json".into(),
        );
    }
    if plugin_toml_names.len() > 1 {
        return Err("plugin archive contains multiple root plugin.toml files".into());
    }
    Ok(ArchiveLayout {
        root_dir: roots.into_iter().next().expect("one root"),
        manifest_name: manifest_names.remove(0),
        plugin_toml_name: plugin_toml_names.pop(),
        files,
    })
}

fn ensure_small_manifest<R: Read>(
    entry: &zip::read::ZipFile<'_, R>,
    name: &str,
) -> Result<(), String> {
    if entry.size() > MAX_MANIFEST_BYTES {
        return Err(format!("{name} exceeds {MAX_MANIFEST_BYTES} bytes"));
    }
    Ok(())
}

fn read_small_text_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<String, String> {
    let mut entry = archive
        .by_name(name)
        .map_err(|error| format!("failed to read {name}: {error}"))?;
    ensure_small_manifest(&entry, name)?;
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|error| format!("failed to read {name}: {error}"))?;
    Ok(text)
}

fn normalize_sha256(value: &str) -> Result<String, String> {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid sha256 digest: {value}"));
    }
    Ok(value.to_ascii_lowercase())
}

fn hash_file(path: &Path) -> Result<String, String> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    hash_reader(file)
}

fn hash_reader(mut reader: impl Read) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("failed to hash input: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn find_momoplug_archives(root: &Path) -> HostResult<Vec<PathBuf>> {
    let mut archives = Vec::new();
    if !root.exists() {
        return Ok(archives);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = fs::read_dir(&dir)
            .map_err(|error| {
                HostError::Config(format!("failed to read {}: {error}", dir.display()))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| HostError::Config(error.to_string()))?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                HostError::Config(format!("failed to inspect {}: {error}", path.display()))
            })?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && path.extension().and_then(|value| value.to_str()) == Some("momoplug")
            {
                archives.push(path);
            }
        }
    }
    archives.sort();
    Ok(archives)
}

#[cfg(test)]
mod tests {
    use super::{hash_reader, scan_momoplug_packages};
    use crate::plugin_path::safe_relative_path;
    use crate::{MutsukiTauriConfig, MutsukiTauriHost, PathsConfig};
    use mutsuki_runtime_contracts::{
        ArtifactType, LifecyclePolicy, PermissionGrant, PluginArtifact, PluginManifest,
        PluginProvides, TaskOutcome,
    };
    use mutsuki_tauri_bridge::FrontendTaskRequest;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs::File;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::OnceLock;
    use uuid::Uuid;
    use zip::write::SimpleFileOptions;

    #[test]
    fn archive_paths_reject_traversal_and_windows_prefixes() {
        for value in [
            "../plugin.dll",
            "root/../plugin.dll",
            "/root/plugin.dll",
            r"root\plugin.dll",
            r"C:\plugin.dll",
        ] {
            assert!(safe_relative_path(value).is_err(), "{value}");
        }
        assert_eq!(
            safe_relative_path("root/bin/plugin.dll").unwrap(),
            std::path::PathBuf::from("root/bin/plugin.dll")
        );
    }

    #[test]
    fn valid_package_stages_only_declared_artifacts_in_hash_directory() {
        let workspace = PackageWorkspace::new("valid");
        let primary = b"abi-v2-plugin";
        let companion = b"office-helper";
        let plugin_toml = plugin_toml(
            "fixture.package",
            "1.2.3",
            "bin/plugin.dll",
            primary,
            Some(("tools/helper.exe", companion, true, "office-helper")),
        );
        write_package(
            &workspace.package_path("valid"),
            &[
                (
                    "fixture/manifest.json",
                    product_manifest("fixture.package", "1.2.3", "native-dylib").as_bytes(),
                ),
                ("fixture/plugin.toml", plugin_toml.as_bytes()),
                ("fixture/bin/plugin.dll", primary),
                ("fixture/tools/helper.exe", companion),
                ("fixture/ui/index.js", b"must-not-be-extracted"),
            ],
        );

        let records = scan_momoplug_packages(&workspace.config).expect("package scan succeeds");

        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert!(record.error.is_none(), "{:?}", record.error);
        let artifact = record.staged_artifact.as_ref().expect("artifact staged");
        assert_eq!(std::fs::read(artifact).unwrap(), primary);
        assert_eq!(
            artifact
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .and_then(|value| value.to_str()),
            Some(record.package_sha256.as_str())
        );
        assert_eq!(record.staged_companions.len(), 1);
        assert_eq!(
            std::fs::read(&record.staged_companions[0].path).unwrap(),
            companion
        );
        assert_eq!(
            record.staged_companions[0].role.as_deref(),
            Some("office-helper")
        );
        assert!(
            !artifact
                .parent()
                .and_then(Path::parent)
                .expect("hash directory")
                .join("ui/index.js")
                .exists()
        );
    }

    #[test]
    fn archive_traversal_invalidates_whole_package_without_extracting() {
        let workspace = PackageWorkspace::new("traversal");
        let plugin_toml = plugin_toml("fixture.traversal", "1.0.0", "plugin.dll", b"plugin", None);
        write_package(
            &workspace.package_path("traversal"),
            &[
                (
                    "fixture/manifest.json",
                    product_manifest("fixture.traversal", "1.0.0", "native-dylib").as_bytes(),
                ),
                ("fixture/plugin.toml", plugin_toml.as_bytes()),
                ("fixture/plugin.dll", b"plugin"),
                ("fixture/../../escape.txt", b"escape"),
            ],
        );

        let records = scan_momoplug_packages(&workspace.config).expect("scan is isolated");

        assert_eq!(records.len(), 1);
        assert!(
            records[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("unsafe plugin archive path"))
        );
        assert!(!workspace.root.join("escape.txt").exists());
        assert!(!workspace.config.paths.cache_dir.exists());
    }

    #[test]
    fn executable_package_requires_matching_dual_manifest_identity() {
        let workspace = PackageWorkspace::new("identity");
        let plugin_toml = plugin_toml("fixture.other", "2.0.0", "plugin.dll", b"plugin", None);
        write_package(
            &workspace.package_path("identity"),
            &[
                (
                    "fixture/manifest.json",
                    product_manifest("fixture.identity", "1.0.0", "native-dylib").as_bytes(),
                ),
                ("fixture/plugin.toml", plugin_toml.as_bytes()),
                ("fixture/plugin.dll", b"plugin"),
            ],
        );

        let records = scan_momoplug_packages(&workspace.config).expect("scan is isolated");

        assert!(
            records[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("manifest identity mismatch"))
        );
    }

    #[test]
    fn declared_hash_mismatch_fails_before_staging() {
        let workspace = PackageWorkspace::new("hash");
        let plugin_toml = plugin_toml("fixture.hash", "1.0.0", "plugin.dll", b"different", None);
        write_package(
            &workspace.package_path("hash"),
            &[
                (
                    "fixture/manifest.json",
                    product_manifest("fixture.hash", "1.0.0", "native-dylib").as_bytes(),
                ),
                ("fixture/plugin.toml", plugin_toml.as_bytes()),
                ("fixture/plugin.dll", b"plugin"),
            ],
        );

        let records = scan_momoplug_packages(&workspace.config).expect("scan is isolated");

        assert!(
            records[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("sha256 mismatch"))
        );
        assert!(!workspace.config.paths.cache_dir.exists());
    }

    #[test]
    fn disabled_package_is_validated_but_not_staged() {
        let mut workspace = PackageWorkspace::new("disabled");
        workspace.config.plugin_selection.enabled_plugin_ids = Some(BTreeSet::new());
        let plugin_toml = plugin_toml("fixture.disabled", "1.0.0", "plugin.dll", b"plugin", None);
        write_package(
            &workspace.package_path("disabled"),
            &[
                (
                    "fixture/manifest.json",
                    product_manifest("fixture.disabled", "1.0.0", "native-dylib").as_bytes(),
                ),
                ("fixture/plugin.toml", plugin_toml.as_bytes()),
                ("fixture/plugin.dll", b"plugin"),
            ],
        );

        let records = scan_momoplug_packages(&workspace.config).expect("scan succeeds");

        assert!(records[0].error.is_none());
        assert!(!records[0].selected);
        assert!(records[0].staged_artifact.is_none());
        assert!(!workspace.config.paths.cache_dir.exists());
    }

    #[test]
    fn real_abi_v2_package_loads_and_executes_through_core() {
        let mut workspace = PackageWorkspace::new("abi-v2");
        workspace.config.plugin_selection.configs.insert(
            mutsuki_tauri_abi_fixture::PLUGIN_ID.into(),
            serde_json::json!({ "enabled": true, "caller": "preserved" }),
        );
        let library = abi_fixture_library();
        let library_bytes = std::fs::read(&library).expect("fixture library read");
        let library_name = library
            .file_name()
            .and_then(|value| value.to_str())
            .expect("fixture library name");
        let artifact_path = format!("bin/{library_name}");
        let artifact_hash = hash_reader(library_bytes.as_slice()).unwrap();
        let manifest = mutsuki_tauri_abi_fixture::fixture_manifest(&artifact_path, &artifact_hash);
        let plugin_toml = toml::to_string(&manifest).expect("fixture manifest serializes");
        let archive_artifact = format!("fixture/{artifact_path}");
        let package_path = workspace.package_path("abi-v2");
        write_package(
            &package_path,
            &[
                (
                    "fixture/manifest.json",
                    product_manifest(
                        mutsuki_tauri_abi_fixture::PLUGIN_ID,
                        &manifest.version,
                        "native-dylib",
                    )
                    .as_bytes(),
                ),
                ("fixture/plugin.toml", plugin_toml.as_bytes()),
                (&archive_artifact, library_bytes.as_slice()),
            ],
        );

        let host = MutsukiTauriHost::builder()
            .config(workspace.config.clone())
            .build()
            .expect("host loads ABI v2 package");
        let result = host
            .call(abi_request("task:abi-v2-package"))
            .expect("ABI v2 task completes");

        assert!(host.plugins().iter().any(|plugin| {
            plugin.plugin_id == mutsuki_tauri_abi_fixture::PLUGIN_ID
                && plugin.deployment == "abi"
                && plugin.status == "loaded"
        }));
        assert!(matches!(
            result.outcome,
            Some(TaskOutcome::Completed { output: Some(output), .. })
                if output == serde_json::json!({ "echo": { "message": "hello" } })
        ));
        std::fs::write(&package_path, b"broken archive").expect("package corrupted");
        assert!(
            host.reload_plugins(host.plugin_selection(), std::time::Duration::from_secs(1))
                .is_err()
        );
        assert!(
            host.call(abi_request("task:abi-v2-after-failed-reload"))
                .is_ok()
        );
    }

    struct PackageWorkspace {
        root: PathBuf,
        config: MutsukiTauriConfig,
    }

    impl PackageWorkspace {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("mutsuki-tauri-package-{name}-{}", Uuid::new_v4()));
            let plugins_dir = root.join("plugins");
            std::fs::create_dir_all(&plugins_dir).expect("plugin directory created");
            let mut config = MutsukiTauriConfig::for_app("PackageTest");
            config.paths = PathsConfig {
                app_data_dir: root.clone(),
                config_dir: root.join("config"),
                data_dir: root.join("data"),
                cache_dir: root.join("cache"),
                logs_dir: root.join("logs"),
                plugins_dir,
                resources_dir: root.join("resources"),
                runners_dir: root.join("runners"),
            };
            Self { root, config }
        }

        fn package_path(&self, name: &str) -> PathBuf {
            self.config
                .paths
                .plugins_dir
                .join(format!("{name}.momoplug"))
        }
    }

    impl Drop for PackageWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn product_manifest(plugin_id: &str, version: &str, runtime: &str) -> String {
        serde_json::json!({
            "pluginId": plugin_id,
            "version": version,
            "runtime": runtime,
            "entry": { "backend": { "library": "plugin" } }
        })
        .to_string()
    }

    fn plugin_toml(
        plugin_id: &str,
        version: &str,
        artifact_path: &str,
        artifact_bytes: &[u8],
        companion: Option<(&str, &[u8], bool, &str)>,
    ) -> String {
        let manifest = PluginManifest {
            plugin_id: plugin_id.into(),
            version: version.into(),
            api_version: "mutsuki-plugin-v1".into(),
            artifact: PluginArtifact {
                artifact_type: ArtifactType::Abi,
                path: artifact_path.into(),
                sha256: hash_reader(artifact_bytes).unwrap(),
                companion_artifacts: Vec::new(),
            },
            provides: PluginProvides::default(),
            requires: Vec::new(),
            permissions: PermissionGrant {
                effects: Vec::new(),
                resources: Vec::new(),
            },
            lifecycle: LifecyclePolicy {
                reload_policy: "drain_and_swap".into(),
                unload_timeout_ms: 5000,
                supports_cancel: true,
                supports_dispose: true,
                supports_snapshot: false,
            },
            metadata: BTreeMap::new(),
        };
        let mut text = toml::to_string(&manifest).expect("manifest serializes");
        if let Some((path, bytes, executable, role)) = companion {
            text.push_str(&format!(
                "\n[[artifact.companion_artifacts]]\npath = {path:?}\nsha256 = {:?}\nexecutable = {executable}\nrole = {role:?}\n",
                hash_reader(bytes).unwrap()
            ));
        }
        text
    }

    fn write_package(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).expect("package created");
        let mut writer = zip::ZipWriter::new(file);
        for (name, bytes) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .expect("entry created");
            writer.write_all(bytes).expect("entry written");
        }
        writer.finish().expect("package finished");
    }

    fn abi_fixture_library() -> PathBuf {
        static LIBRARY: OnceLock<PathBuf> = OnceLock::new();
        LIBRARY
            .get_or_init(|| {
                let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .ancestors()
                    .find(|path| {
                        path.join("Cargo.toml").is_file() && path.join("Cargo.lock").is_file()
                    })
                    .expect("host crate is inside a locked workspace");
                let target = workspace.join("target/test-fixtures");
                let status = Command::new(env!("CARGO"))
                    .args([
                        "build",
                        "--locked",
                        "--manifest-path",
                        workspace.join("Cargo.toml").to_str().unwrap(),
                        "--target-dir",
                        target.to_str().unwrap(),
                        "-p",
                        "mutsuki-tauri-abi-fixture",
                    ])
                    .status()
                    .expect("build ABI fixture plugin");
                assert!(status.success(), "ABI fixture plugin build failed");
                target.join("debug").join(if cfg!(windows) {
                    "mutsuki_tauri_abi_fixture.dll"
                } else if cfg!(target_os = "macos") {
                    "libmutsuki_tauri_abi_fixture.dylib"
                } else {
                    "libmutsuki_tauri_abi_fixture.so"
                })
            })
            .clone()
    }

    fn abi_request(task_id: &str) -> FrontendTaskRequest {
        FrontendTaskRequest {
            protocol_id: mutsuki_tauri_abi_fixture::PROTOCOL_ID.into(),
            payload: serde_json::json!({ "message": "hello" }),
            task_id: Some(task_id.into()),
            trace_id: None,
            correlation_id: None,
            idempotency_key: None,
            target_binding_id: None,
            runner_hint: None,
            input_refs: Vec::new(),
            priority: 0,
            context: Default::default(),
        }
    }
}
