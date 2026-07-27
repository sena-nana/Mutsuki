use mutsuki_runtime_contracts::ResourceRef;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const VERSION: &str = "0.1.0";
pub const ABI_CODEC: &str = "serde-json";
pub const SNAPSHOT: &str = "mutsuki.browser.snapshot";
pub const PROTOCOL_IDS: &[&str] = &[SNAPSHOT];
pub const SNAPSHOT_SCHEMA: &str = "mutsuki.browser.snapshot.output.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserWaitMode {
    Load,
    Selector,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowserSnapshotRequest {
    pub url: String,
    pub output_resource: ResourceRef,
    pub wait_mode: BrowserWaitMode,
    pub selector: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserSnapshot {
    pub final_url: String,
    pub title: String,
    pub html: String,
}

pub fn input_schema(protocol_id: &str) -> Option<Value> {
    (protocol_id == SNAPSHOT).then(|| {
        json!({
            "type": "object",
            "required": ["url", "output_resource", "wait_mode", "selector", "timeout_ms"],
            "properties": {
                "url": {"type": "string"},
                "output_resource": {"type": "object"},
                "wait_mode": {"enum": ["load", "selector"]},
                "selector": {"type": ["string", "null"]},
                "timeout_ms": {"type": "integer", "minimum": 1}
            }
        })
    })
}

pub fn output_schema(protocol_id: &str) -> Option<Value> {
    (protocol_id == SNAPSHOT).then(|| {
        json!({
            "type": "object",
            "required": ["final_url", "title", "html"]
        })
    })
}

pub fn error_schema(protocol_id: &str) -> Option<Value> {
    (protocol_id == SNAPSHOT).then(|| json!({"type": "object"}))
}

#[cfg(test)]
mod tests {
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };

    use super::*;

    #[test]
    fn request_round_trips_with_explicit_wait_fields() {
        let request = BrowserSnapshotRequest {
            url: "https://www.mihuashi.com/profiles/449216?role=painter".into(),
            output_resource: ResourceRef {
                ref_id: "snapshot-1".into(),
                resource_id: ResourceId {
                    kind_id: "browser.snapshot".into(),
                    slot_id: "snapshot-1".into(),
                    generation: 1,
                    version: 1,
                },
                semantic: ResourceSemantic::CowVersionedState,
                provider_id: "mutsuki.std.resource.memory".into(),
                resource_kind: "browser.snapshot".into(),
                schema: SNAPSHOT_SCHEMA.into(),
                version: 1,
                generation: 1,
                access: ResourceAccess::ProviderRpc {
                    provider_id: "mutsuki.std.resource.memory".into(),
                    method: "memory".into(),
                },
                size_hint: Some(0),
                content_hash: None,
                lifetime: ResourceLifetime::Persistent,
                lease: None,
                seal_state: ResourceSealState::Sealed,
            },
            wait_mode: BrowserWaitMode::Selector,
            selector: Some("main".into()),
            timeout_ms: 5_000,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            serde_json::from_value::<BrowserSnapshotRequest>(value).unwrap(),
            request
        );
    }
}
