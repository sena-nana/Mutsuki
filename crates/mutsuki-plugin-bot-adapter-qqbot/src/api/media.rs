use std::sync::Arc;

use mutsuki_runtime_contracts::{ReadPlan, ResourceRef};
use mutsuki_runtime_sdk::ResourceRegistryGateway;
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaChunk {
    pub index: u64,
    pub bytes: Vec<u8>,
    pub md5: String,
}

pub trait QqMediaProvider: Send {
    fn read_chunks(
        &mut self,
        resource: &ResourceRef,
        block_size: u64,
    ) -> Result<Vec<MediaChunk>, QqMediaError>;
}

pub struct ResourceGatewayQqMediaProvider {
    provider_id: String,
    resources: Arc<dyn ResourceRegistryGateway>,
}

impl ResourceGatewayQqMediaProvider {
    pub fn new(
        provider_id: impl Into<String>,
        resources: Arc<dyn ResourceRegistryGateway>,
    ) -> Result<Self, QqMediaError> {
        let provider_id = provider_id.into();
        if provider_id.trim().is_empty() {
            return Err(QqMediaError::NotReadable(
                "media provider id must be explicit".into(),
            ));
        }
        Ok(Self {
            provider_id,
            resources,
        })
    }
}

impl QqMediaProvider for ResourceGatewayQqMediaProvider {
    fn read_chunks(
        &mut self,
        resource: &ResourceRef,
        block_size: u64,
    ) -> Result<Vec<MediaChunk>, QqMediaError> {
        if resource.provider_id != self.provider_id {
            return Err(QqMediaError::NotReadable(format!(
                "resource provider {} does not match configured provider {}",
                resource.provider_id, self.provider_id
            )));
        }
        if block_size == 0 {
            return Err(QqMediaError::NotReadable(
                "QQ upload block size must be positive".into(),
            ));
        }
        let latest = self
            .resources
            .open_resource_descriptor(&resource.ref_id)
            .map_err(|error| QqMediaError::NotFound(error.to_string()))?;
        if latest.provider_id != self.provider_id || latest.generation != resource.generation {
            return Err(QqMediaError::NotReadable(
                "resource descriptor is stale or belongs to another provider".into(),
            ));
        }
        let bytes = self
            .resources
            .collect_read_plan(&ReadPlan {
                plan_id: format!("qq.media.collect.{}", latest.ref_id),
                resource: latest.clone(),
                operation: "collect".into(),
                args: Value::Null,
            })
            .map_err(|error| QqMediaError::Failed(error.to_string()))?;
        if let Some(expected) = latest.content_hash.as_deref() {
            let actual = format!("sha256:{:x}", Sha256::digest(&bytes));
            if expected != actual {
                return Err(QqMediaError::Failed(format!(
                    "resource digest mismatch: expected {expected}, got {actual}"
                )));
            }
        }
        Ok(bytes
            .chunks(block_size as usize)
            .enumerate()
            .map(|(index, bytes)| MediaChunk {
                index: index as u64,
                bytes: bytes.to_vec(),
                md5: format!("{:x}", md5::compute(bytes)),
            })
            .collect())
    }
}

#[derive(Debug, Error)]
pub enum QqMediaError {
    #[error("media resource not found: {0}")]
    NotFound(String),
    #[error("media resource is not readable: {0}")]
    NotReadable(String),
    #[error("media resource failed: {0}")]
    Failed(String),
}
