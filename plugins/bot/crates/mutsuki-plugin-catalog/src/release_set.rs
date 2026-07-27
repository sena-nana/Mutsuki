use std::path::Path;

use serde::Deserialize;

use crate::{CatalogError, CatalogResult};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ReleaseSetRepository {
    pub id: String,
    pub url: String,
    pub revision: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ReleaseSetInfo {
    pub schema_version: u32,
    pub release: String,
    pub status: String,
    pub contracts_api: String,
    pub runtime_wire_schema: String,
    #[serde(default)]
    pub supported_deployments: Vec<String>,
    #[serde(default)]
    pub unsupported_deployments: Vec<String>,
    #[serde(default)]
    pub repositories: Vec<ReleaseSetRepository>,
}

pub fn load_release_set(path: &Path) -> CatalogResult<ReleaseSetInfo> {
    let text = std::fs::read_to_string(path).map_err(|source| CatalogError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let info: ReleaseSetInfo = toml::from_str(&text)
        .map_err(|error| CatalogError::ReleaseSet(format!("{}: {error}", path.display())))?;
    if info.schema_version != 1 {
        return Err(CatalogError::ReleaseSet(format!(
            "{}: schema_version must be 1",
            path.display()
        )));
    }
    Ok(info)
}
