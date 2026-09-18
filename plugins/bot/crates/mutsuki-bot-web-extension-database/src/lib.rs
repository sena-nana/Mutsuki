//! Database WebExtension: browse the live Bot state SQLite catalog.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_possible_truncation,
    clippy::doc_markdown,
    clippy::needless_pass_by_value
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_bot_state_db::{BotStateDbError, BotStateDbRepository};
use mutsuki_web_extension_api::{
    BundledAssets, BundledManifest, ExtensionError, RpcRegistry, WebExtension,
    WebExtensionDescriptor, materialize_bundled_assets,
};
use mutsuki_web_protocol::{AssetEntry, ExtensionManifest, WebFrontendAssets};
use serde_json::Value;

pub const PLUGIN_ID: &str = "database";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const CAPABILITY_RUNTIME_READ: &str = "runtime.read";

pub struct DatabaseWebExtension {
    repository: Option<Arc<BotStateDbRepository>>,
    assets_root: BundledAssets,
}

impl DatabaseWebExtension {
    #[must_use]
    pub fn new(repository: Option<Arc<BotStateDbRepository>>) -> Self {
        Self {
            repository,
            assets_root: BundledAssets::default(),
        }
    }

    #[must_use]
    pub fn with_frontend_assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.assets_root.set(root);
        self
    }
}

impl WebExtension for DatabaseWebExtension {
    fn descriptor(&self) -> WebExtensionDescriptor {
        manifest(
            self.frontend_assets()
                .map(|assets| assets.manifest.assets)
                .unwrap_or_default(),
        )
    }

    fn frontend_assets(&self) -> Option<WebFrontendAssets> {
        self.assets_root.resolve(manifest)
    }

    fn register_rpc(&self, registry: &mut RpcRegistry) -> Result<(), ExtensionError> {
        let repository = self.repository.clone();
        registry.register_contextual("snapshot", {
            let repository = repository.clone();
            move |context, _params| {
                context.require(CAPABILITY_RUNTIME_READ)?;
                match &repository {
                    Some(repository) => {
                        serde_json::to_value(repository.inspect_snapshot().map_err(map_db_error)?)
                            .map_err(|error| {
                                ExtensionError::Registration(format!(
                                    "snapshot encode failed: {error}"
                                ))
                            })
                    }
                    None => Ok(Value::Null),
                }
            }
        });
        registry.register_contextual("rows", move |context, params| {
            context.require(CAPABILITY_RUNTIME_READ)?;
            let repository = repository.as_ref().ok_or_else(unavailable)?;
            let table = params
                .get("table")
                .and_then(Value::as_str)
                .ok_or_else(|| ExtensionError::Registration("missing table".into()))?;
            let after = params.get("after").and_then(Value::as_str);
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50) as u32;
            serde_json::to_value(
                repository
                    .inspect_rows(table, after, limit)
                    .map_err(map_db_error)?,
            )
            .map_err(|error| ExtensionError::Registration(format!("rows encode failed: {error}")))
        });
        Ok(())
    }

    fn register_events(
        &self,
        _registry: &mut mutsuki_web_extension_api::EventRegistry,
    ) -> Result<(), ExtensionError> {
        Ok(())
    }
}

/// Writes the embedded database frontend and its content-addressed manifest.
///
/// # Errors
///
/// Returns an I/O error when directory creation, manifest encoding, or asset writes fail.
pub fn materialize_frontend_assets(out_dir: &Path) -> Result<PathBuf, std::io::Error> {
    let js = include_str!("../assets/index.js");
    materialize_bundled_assets(out_dir, manifest, &[("index.js", js.as_bytes())])
}

fn manifest(assets: Vec<AssetEntry>) -> ExtensionManifest {
    BundledManifest {
        id: PLUGIN_ID,
        version: PLUGIN_VERSION,
        entry: "index.js",
        capabilities: vec![CAPABILITY_RUNTIME_READ.into()],
    }
    .build(assets)
}

fn unavailable() -> ExtensionError {
    ExtensionError::Rpc {
        code: "database.unavailable".into(),
        message: "当前没有已接入的数据库".into(),
    }
}

fn map_db_error(error: BotStateDbError) -> ExtensionError {
    ExtensionError::Rpc {
        code: if matches!(error, BotStateDbError::InvalidConfiguration(_)) {
            "database.invalid_request"
        } else {
            "database.inspect_failed"
        }
        .into(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    /// Stored cell text reaches attribute values, so the escaper is a security
    /// boundary rather than a formatting detail.
    #[test]
    fn cell_escaping_closes_attribute_injection() {
        let script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/escape.test.mjs");
        let output = std::process::Command::new("node")
            .arg(&script)
            .output()
            .expect("node must be available to verify database cell escaping");
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
