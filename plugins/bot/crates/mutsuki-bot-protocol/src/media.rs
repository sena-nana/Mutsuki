use mutsuki_runtime_contracts::ResourceRef;
use serde::{Deserialize, Serialize};

use crate::BotTarget;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotMediaKind {
    Image,
    Video,
    Audio,
    File,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotMediaUploadRequest {
    pub target: BotTarget,
    pub kind: BotMediaKind,
    pub resource: ResourceRef,
    pub file_name: Option<String>,
}
