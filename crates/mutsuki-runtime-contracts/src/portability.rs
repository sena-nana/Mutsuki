use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use crate::{
    ERR_CHECKPOINT_INCOMPATIBLE, ERR_PORTABLE_SCHEMA_UNSUPPORTED, ProtocolId, RefId, RuntimeError,
    ScalarValue, Task,
};

pub const PORTABLE_TASK_ENVELOPE_SCHEMA_ID: &str = "mutsuki.runtime.portable-task";
pub const PORTABLE_TASK_ENVELOPE_SCHEMA_VERSION: &str = "1.0.0";
pub const TASK_CHECKPOINT_ENVELOPE_SCHEMA_ID: &str = "mutsuki.runtime.task-checkpoint";
pub const TASK_CHECKPOINT_ENVELOPE_SCHEMA_VERSION: &str = "1.0.0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaIdentity {
    pub schema_id: String,
    pub schema_version: String,
}

impl SchemaIdentity {
    pub fn new(schema_id: impl Into<String>, schema_version: impl Into<String>) -> Self {
        Self {
            schema_id: schema_id.into(),
            schema_version: schema_version.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentId {
    pub algorithm: String,
    pub digest: String,
    pub size: u64,
    pub format: String,
}

impl ContentId {
    pub fn new(
        algorithm: impl Into<String>,
        digest: impl Into<String>,
        size: u64,
        format: impl Into<String>,
    ) -> Self {
        Self {
            algorithm: algorithm.into(),
            digest: digest.into(),
            size,
            format: format.into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMobility {
    #[default]
    LocalOnly,
    Portable,
    Restartable,
    Checkpointable,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrySafety {
    Idempotent,
    Verifiable,
    Compensatable,
    #[default]
    Unsafe,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAcceptanceDurability {
    #[default]
    Volatile,
    Buffered,
    Persisted,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePersistence {
    #[default]
    Ephemeral,
    Durable,
    ContentAddressed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMode {
    #[default]
    Unavailable,
    RestartFromInput,
    RestoreCheckpoint,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortabilityCapability {
    pub mobility: ExecutionMobility,
    pub retry_safety: RetrySafety,
    pub task_acceptance: TaskAcceptanceDurability,
    pub resource_persistence: ResourcePersistence,
    pub recovery: RecoveryMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPortabilityDescriptor {
    pub protocol_id: ProtocolId,
    pub task_schema: SchemaIdentity,
    pub checkpoint_schema: Option<SchemaIdentity>,
    pub capability: PortabilityCapability,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortableResourceDescriptor {
    pub task_ref_id: RefId,
    pub content_id: ContentId,
    pub resource_kind: String,
    pub schema: SchemaIdentity,
    pub persistence: ResourcePersistence,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortabilityCatalog {
    pub tasks: Vec<TaskPortabilityDescriptor>,
    pub resources: Vec<PortableResourceDescriptor>,
}

impl PortabilityCatalog {
    pub fn task_capability(&self, protocol_id: &str) -> PortabilityCapability {
        self.tasks
            .iter()
            .find(|descriptor| descriptor.protocol_id == protocol_id)
            .map(|descriptor| descriptor.capability.clone())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PortableTask {
    pub envelope_schema: SchemaIdentity,
    pub task_schema: SchemaIdentity,
    pub capability: PortabilityCapability,
    pub input_content_id: ContentId,
    pub resources: Vec<PortableResourceDescriptor>,
    pub task: Task,
}

impl PortableTask {
    pub fn new(
        mut task: Task,
        task_schema: SchemaIdentity,
        input_content_id: ContentId,
        capability: PortabilityCapability,
    ) -> Self {
        clear_runtime_attempt(&mut task);
        Self {
            envelope_schema: SchemaIdentity::new(
                PORTABLE_TASK_ENVELOPE_SCHEMA_ID,
                PORTABLE_TASK_ENVELOPE_SCHEMA_VERSION,
            ),
            task_schema,
            capability,
            input_content_id,
            resources: Vec::new(),
            task,
        }
    }

    pub fn with_resources(mut self, resources: Vec<PortableResourceDescriptor>) -> Self {
        self.resources = resources;
        self
    }

    pub fn has_supported_envelope(&self) -> bool {
        self.envelope_schema.schema_id == PORTABLE_TASK_ENVELOPE_SCHEMA_ID
            && self.envelope_schema.schema_version == PORTABLE_TASK_ENVELOPE_SCHEMA_VERSION
    }

    #[allow(clippy::result_large_err)]
    pub fn validate_envelope(&self) -> Result<(), RuntimeError> {
        if self.has_supported_envelope() {
            return Ok(());
        }
        let mut error = RuntimeError::new(
            ERR_PORTABLE_SCHEMA_UNSUPPORTED,
            "runtime.portability",
            "portable_task.decode",
        );
        error.evidence.insert(
            "schema_id".into(),
            ScalarValue::String(self.envelope_schema.schema_id.clone()),
        );
        error.evidence.insert(
            "schema_version".into(),
            ScalarValue::String(self.envelope_schema.schema_version.clone()),
        );
        Err(error)
    }

    pub fn write_json(&self, writer: impl Write) -> serde_json::Result<()> {
        serde_json::to_writer(writer, self)
    }

    pub fn read_json(reader: impl Read) -> serde_json::Result<Self> {
        serde_json::from_reader(reader)
    }

    pub fn into_local_task(mut self) -> Task {
        clear_runtime_attempt(&mut self.task);
        self.task
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskCheckpoint {
    pub envelope_schema: SchemaIdentity,
    pub checkpoint_schema: SchemaIdentity,
    pub task_schema: SchemaIdentity,
    pub implementation_generation: u64,
    pub input_content_id: ContentId,
    pub sequence: u64,
    pub task: PortableTask,
    pub payload: Vec<u8>,
}

impl TaskCheckpoint {
    pub fn new(
        checkpoint_schema: SchemaIdentity,
        implementation_generation: u64,
        sequence: u64,
        task: PortableTask,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            envelope_schema: SchemaIdentity::new(
                TASK_CHECKPOINT_ENVELOPE_SCHEMA_ID,
                TASK_CHECKPOINT_ENVELOPE_SCHEMA_VERSION,
            ),
            checkpoint_schema,
            task_schema: task.task_schema.clone(),
            implementation_generation,
            input_content_id: task.input_content_id.clone(),
            sequence,
            task,
            payload,
        }
    }

    pub fn write_json(&self, writer: impl Write) -> serde_json::Result<()> {
        serde_json::to_writer(writer, self)
    }

    pub fn read_json(reader: impl Read) -> serde_json::Result<Self> {
        serde_json::from_reader(reader)
    }

    pub fn is_compatible_with(
        &self,
        task_schema: &SchemaIdentity,
        implementation_generation: u64,
        input_content_id: &ContentId,
    ) -> bool {
        self.is_self_consistent()
            && self.task_schema == *task_schema
            && self.implementation_generation == implementation_generation
            && self.input_content_id == *input_content_id
    }

    pub fn is_self_consistent(&self) -> bool {
        self.envelope_schema.schema_id == TASK_CHECKPOINT_ENVELOPE_SCHEMA_ID
            && self.envelope_schema.schema_version == TASK_CHECKPOINT_ENVELOPE_SCHEMA_VERSION
            && self.task.has_supported_envelope()
            && self.task_schema == self.task.task_schema
            && self.input_content_id == self.task.input_content_id
    }

    #[allow(clippy::result_large_err)]
    pub fn validate_restore(
        &self,
        task_schema: &SchemaIdentity,
        implementation_generation: u64,
        input_content_id: &ContentId,
    ) -> Result<(), RuntimeError> {
        if self.is_compatible_with(task_schema, implementation_generation, input_content_id) {
            return Ok(());
        }
        let mut error = RuntimeError::new(
            ERR_CHECKPOINT_INCOMPATIBLE,
            "runtime.portability",
            "checkpoint.restore",
        );
        error.recovery = Some("restart_from_input_or_reject".into());
        error.evidence.insert(
            "checkpoint_sequence".into(),
            ScalarValue::Int(self.sequence as i64),
        );
        error.evidence.insert(
            "checkpoint_generation".into(),
            ScalarValue::Int(self.implementation_generation as i64),
        );
        error.evidence.insert(
            "requested_generation".into(),
            ScalarValue::Int(implementation_generation as i64),
        );
        Err(error)
    }

    pub fn restart_from_input(&self) -> Task {
        self.task.clone().into_local_task()
    }
}

fn clear_runtime_attempt(task: &mut Task) {
    task.lease_id = None;
    task.registry_generation = 0;
    task.created_sequence = 0;
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use serde_json::json;

    use super::*;

    fn portable_task() -> PortableTask {
        let mut task = Task::new("recorded-task", "demo.portable", json!({"value": 7}));
        task.lease_id = Some("local-attempt".into());
        task.registry_generation = 9;
        task.created_sequence = 42;
        PortableTask::new(
            task,
            SchemaIdentity::new("demo.portable.input", "1.0.0"),
            ContentId::new("sha256", "abc123", 11, "application/json"),
            PortabilityCapability {
                mobility: ExecutionMobility::Checkpointable,
                retry_safety: RetrySafety::Idempotent,
                task_acceptance: TaskAcceptanceDurability::Persisted,
                resource_persistence: ResourcePersistence::ContentAddressed,
                recovery: RecoveryMode::RestoreCheckpoint,
            },
        )
    }

    #[test]
    fn portable_task_stream_roundtrip_rebuilds_an_ordinary_local_task() {
        let recorded = portable_task();
        let mut bytes = Vec::new();
        recorded.write_json(&mut bytes).unwrap();
        let decoded = PortableTask::read_json(Cursor::new(bytes)).unwrap();
        let replayed = decoded.into_local_task();

        assert_eq!(replayed.task_id, "recorded-task");
        assert_eq!(replayed.payload, json!({"value": 7}));
        assert_eq!(replayed.lease_id, None);
        assert_eq!(replayed.registry_generation, 0);
        assert_eq!(replayed.created_sequence, 0);

        let mut unsupported = recorded;
        unsupported.envelope_schema.schema_version = "2.0.0".into();
        assert_eq!(
            unsupported.validate_envelope().unwrap_err().code,
            ERR_PORTABLE_SCHEMA_UNSUPPORTED
        );
    }

    #[test]
    fn checkpoint_stream_roundtrip_checks_schema_generation_and_input_digest() {
        let task = portable_task();
        let checkpoint = TaskCheckpoint::new(
            SchemaIdentity::new("demo.portable.checkpoint", "2.0.0"),
            3,
            8,
            task.clone(),
            vec![1, 2, 3],
        );
        let mut bytes = Vec::new();
        checkpoint.write_json(&mut bytes).unwrap();
        let decoded = TaskCheckpoint::read_json(Cursor::new(bytes)).unwrap();

        assert!(decoded.is_compatible_with(&task.task_schema, 3, &task.input_content_id));
        assert!(decoded.is_self_consistent());
        assert!(!decoded.is_compatible_with(&task.task_schema, 4, &task.input_content_id));
        assert_eq!(
            decoded
                .validate_restore(&task.task_schema, 4, &task.input_content_id)
                .unwrap_err()
                .code,
            ERR_CHECKPOINT_INCOMPATIBLE
        );
        assert_eq!(decoded.restart_from_input().protocol_id, "demo.portable");
    }

    #[test]
    fn portable_resources_use_content_identity_until_a_host_materializes_local_refs() {
        let resource = PortableResourceDescriptor {
            task_ref_id: "input:blob".into(),
            content_id: ContentId::new("sha256", "blob-digest", 4, "application/octet-stream"),
            resource_kind: "blob".into(),
            schema: SchemaIdentity::new("demo.blob", "1.0.0"),
            persistence: ResourcePersistence::ContentAddressed,
        };
        let task = portable_task().with_resources(vec![resource.clone()]);

        assert_eq!(task.resources, vec![resource]);
        assert!(task.has_supported_envelope());
        assert_eq!(task.into_local_task().lease_id, None);
    }

    #[test]
    fn missing_capability_is_local_only_and_portable_descriptors_have_no_location_fields() {
        assert_eq!(
            PortabilityCatalog::default()
                .task_capability("legacy.protocol")
                .mobility,
            ExecutionMobility::LocalOnly
        );
        let encoded = serde_json::to_value(PortableResourceDescriptor {
            task_ref_id: "input:blob".into(),
            content_id: ContentId::new("sha256", "abc123", 11, "application/octet-stream"),
            resource_kind: "blob".into(),
            schema: SchemaIdentity::new("demo.blob", "1.0.0"),
            persistence: ResourcePersistence::ContentAddressed,
        })
        .unwrap();
        let forbidden = [
            "node_id", "address", "endpoint", "leader", "quorum", "replica", "network",
        ];
        assert_no_forbidden_keys(&encoded, &forbidden);
    }

    fn assert_no_forbidden_keys(value: &serde_json::Value, forbidden: &[&str]) {
        match value {
            serde_json::Value::Object(object) => {
                for (key, value) in object {
                    assert!(!forbidden.contains(&key.as_str()), "forbidden key: {key}");
                    assert_no_forbidden_keys(value, forbidden);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    assert_no_forbidden_keys(value, forbidden);
                }
            }
            _ => {}
        }
    }
}
