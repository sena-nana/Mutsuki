use std::fs;
use std::path::Path;

use mutsuki_web_protocol::{
    AssetEntry, EXTENSION_MANIFEST_VERSION, ExtensionManifest, ProtocolError, WEB_PROTOCOL_VERSION,
    WEB_PROTOCOL_VERSION_MAJOR,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::ExtensionError;

/// Entry asset every bundled extension ships.
pub const BUNDLED_ENTRY_ASSET: &str = "index.js";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ManifestError {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("manifest io error: {0}")]
    Io(String),
    #[error("manifest parse error: {0}")]
    Parse(String),
    #[error("asset hash mismatch for {path}: expected={expected}, actual={actual}")]
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("missing entry asset: {0}")]
    MissingEntry(String),
    #[error("untrusted extension source: {0}")]
    UntrustedSource(String),
}

/// Load and validate an extension manifest from `root_dir/manifest.json`.
pub fn load_manifest(root_dir: &Path) -> Result<ExtensionManifest, ManifestError> {
    let path = root_dir.join("manifest.json");
    let bytes = fs::read(&path).map_err(|err| ManifestError::Io(err.to_string()))?;
    let manifest: ExtensionManifest =
        serde_json::from_slice(&bytes).map_err(|err| ManifestError::Parse(err.to_string()))?;
    validate_manifest(&manifest, root_dir)?;
    Ok(manifest)
}

/// Resolves the manifest for an extension's materialized asset directory.
///
/// Extensions that ship more than the entry file write a `manifest.json` alongside their assets;
/// single-file extensions do not, so their manifest is derived here from the entry's own bytes.
/// `derive` supplies the extension's identity, capabilities and permissions, which only the
/// extension itself knows.
pub fn load_bundled_manifest(
    root_dir: &Path,
    derive: impl FnOnce(Vec<AssetEntry>) -> ExtensionManifest,
) -> Result<ExtensionManifest, ExtensionError> {
    let declared = root_dir.join("manifest.json");
    if declared.exists() {
        let bytes = fs::read(&declared).map_err(manifest_error)?;
        return serde_json::from_slice(&bytes).map_err(manifest_error);
    }
    let bytes = fs::read(root_dir.join(BUNDLED_ENTRY_ASSET)).map_err(manifest_error)?;
    Ok(derive(vec![AssetEntry {
        path: BUNDLED_ENTRY_ASSET.into(),
        content_hash: content_hash(&bytes),
        bytes: bytes.len() as u64,
    }]))
}

fn manifest_error(error: impl std::fmt::Display) -> ExtensionError {
    ExtensionError::Manifest(error.to_string())
}

/// Validate manifest version, protocol compatibility, entry and content hashes.
pub fn validate_manifest(
    manifest: &ExtensionManifest,
    root_dir: &Path,
) -> Result<(), ManifestError> {
    if manifest.manifest_version != EXTENSION_MANIFEST_VERSION {
        return Err(ManifestError::Protocol(
            ProtocolError::UnsupportedManifestVersion(manifest.manifest_version),
        ));
    }

    if !manifest.protocol_version.is_empty() {
        ensure_protocol_compatible(&manifest.protocol_version)?;
    }

    let entry_path = root_dir.join(&manifest.entry);
    if !entry_path.is_file() {
        return Err(ManifestError::MissingEntry(manifest.entry.clone()));
    }

    for asset in &manifest.assets {
        let asset_path = root_dir.join(&asset.path);
        let bytes = fs::read(&asset_path).map_err(|err| ManifestError::Io(err.to_string()))?;
        if bytes.len() as u64 != asset.bytes {
            return Err(ManifestError::HashMismatch {
                path: asset.path.clone(),
                expected: format!("{} bytes", asset.bytes),
                actual: format!("{} bytes", bytes.len()),
            });
        }
        let actual = content_hash(&bytes);
        if actual != asset.content_hash {
            return Err(ManifestError::HashMismatch {
                path: asset.path.clone(),
                expected: asset.content_hash.clone(),
                actual,
            });
        }
    }

    Ok(())
}

fn ensure_protocol_compatible(client: &str) -> Result<(), ManifestError> {
    let major = client
        .split('.')
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .ok_or_else(|| {
            ManifestError::Protocol(ProtocolError::VersionMismatch {
                client: client.to_string(),
                host: WEB_PROTOCOL_VERSION.to_string(),
            })
        })?;
    if major != WEB_PROTOCOL_VERSION_MAJOR {
        return Err(ManifestError::Protocol(ProtocolError::VersionMismatch {
            client: client.to_string(),
            host: WEB_PROTOCOL_VERSION.to_string(),
        }));
    }
    Ok(())
}

pub fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", hex_encode(&digest))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_web_protocol::{AssetEntry, EXTENSION_MANIFEST_VERSION, WEB_PROTOCOL_VERSION};
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn rejects_unsupported_manifest_version() {
        let dir = tempdir().unwrap();
        let entry = dir.path().join("index.js");
        std::fs::write(&entry, b"export default {}").unwrap();
        let manifest = ExtensionManifest {
            manifest_version: 99,
            id: "example".into(),
            version: "0.1.0".into(),
            entry: "index.js".into(),
            capabilities: vec![],
            permissions: vec![],
            assets: vec![],
            protocol_version: WEB_PROTOCOL_VERSION.into(),
        };
        let err = validate_manifest(&manifest, dir.path()).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::Protocol(ProtocolError::UnsupportedManifestVersion(99))
        ));
        let _ = EXTENSION_MANIFEST_VERSION;
    }

    #[test]
    fn bundled_manifest_prefers_the_declared_file_over_the_derived_entry() {
        let dir = tempdir().unwrap();
        let entry = b"export default { id: 'derived' }";
        std::fs::write(dir.path().join(BUNDLED_ENTRY_ASSET), entry).unwrap();
        let derive = |assets: Vec<AssetEntry>| ExtensionManifest {
            manifest_version: EXTENSION_MANIFEST_VERSION,
            id: "example".into(),
            version: "0.1.0".into(),
            entry: BUNDLED_ENTRY_ASSET.into(),
            capabilities: vec![],
            permissions: vec![],
            assets,
            protocol_version: WEB_PROTOCOL_VERSION.into(),
        };

        let derived = load_bundled_manifest(dir.path(), derive).unwrap();
        assert_eq!(derived.assets.len(), 1);
        assert_eq!(derived.assets[0].path, BUNDLED_ENTRY_ASSET);
        assert_eq!(derived.assets[0].content_hash, content_hash(entry));
        assert_eq!(derived.assets[0].bytes, entry.len() as u64);

        let mut declared = derive(Vec::new());
        declared.version = "9.9.9".into();
        std::fs::write(
            dir.path().join("manifest.json"),
            serde_json::to_vec(&declared).unwrap(),
        )
        .unwrap();
        let loaded = load_bundled_manifest(dir.path(), derive).unwrap();
        assert_eq!(loaded.version, "9.9.9");
        assert!(loaded.assets.is_empty());
    }

    #[test]
    fn bundled_manifest_reports_a_missing_entry_asset() {
        let dir = tempdir().unwrap();
        let error = load_bundled_manifest(dir.path(), |_| unreachable!("entry is absent"))
            .expect_err("no entry asset");
        assert!(matches!(error, ExtensionError::Manifest(_)));
    }

    #[test]
    fn validates_content_hash() {
        let dir = tempdir().unwrap();
        let entry = dir.path().join("index.js");
        let bytes = b"export default { id: 'ok' }";
        let mut file = std::fs::File::create(&entry).unwrap();
        file.write_all(bytes).unwrap();
        let hash = content_hash(bytes);
        let manifest = ExtensionManifest {
            manifest_version: EXTENSION_MANIFEST_VERSION,
            id: "example".into(),
            version: "0.1.0".into(),
            entry: "index.js".into(),
            capabilities: vec!["example.read".into()],
            permissions: vec![],
            assets: vec![AssetEntry {
                path: "index.js".into(),
                content_hash: hash,
                bytes: bytes.len() as u64,
            }],
            protocol_version: WEB_PROTOCOL_VERSION.into(),
        };
        validate_manifest(&manifest, dir.path()).unwrap();
    }
}
