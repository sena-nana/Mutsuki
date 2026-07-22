use std::fs;
use std::path::{Path, PathBuf};

use mutsuki_runtime_contracts::{
    CompletionBatch, DomainEvent, ExecutionClass, RunnerBatchCapability, RunnerContext,
    RunnerDescriptor, RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect, RuntimeError,
    ScalarValue, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    LoadedPlugin, Plugin, PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder,
    map_work_batch_entries,
};
use mutsuki_std_effect::{
    EffectObservation, ProtocolPair, ProtocolPairTable, derive_effect_from_pair,
};
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.std.io.fs";
pub const RUNNER_ID: &str = "mutsuki.std.io.fs.runner";
pub const EFFECT_RUNNER_ID: &str = "effect.mutsuki.std.io.fs.runner";

pub const FS_READ_PROTOCOL: &str = "mutsuki.fs.read";
pub const FS_WRITE_PROTOCOL: &str = "mutsuki.fs.write";
pub const FS_APPEND_PROTOCOL: &str = "mutsuki.fs.append";
pub const FS_COPY_PROTOCOL: &str = "mutsuki.fs.copy";
pub const FS_MOVE_PROTOCOL: &str = "mutsuki.fs.move";
pub const FS_REMOVE_PROTOCOL: &str = "mutsuki.fs.remove";
pub const FS_MKDIR_PROTOCOL: &str = "mutsuki.fs.mkdir";
pub const FS_LIST_PROTOCOL: &str = "mutsuki.fs.list";
pub const FS_STAT_PROTOCOL: &str = "mutsuki.fs.stat";
pub const FS_EXISTS_PROTOCOL: &str = "mutsuki.fs.exists";

pub const EFFECT_FS_READ_PROTOCOL: &str = "effect.mutsuki.fs.read";
pub const EFFECT_FS_WRITE_PROTOCOL: &str = "effect.mutsuki.fs.write";
pub const EFFECT_FS_APPEND_PROTOCOL: &str = "effect.mutsuki.fs.append";
pub const EFFECT_FS_COPY_PROTOCOL: &str = "effect.mutsuki.fs.copy";
pub const EFFECT_FS_MOVE_PROTOCOL: &str = "effect.mutsuki.fs.move";
pub const EFFECT_FS_REMOVE_PROTOCOL: &str = "effect.mutsuki.fs.remove";
pub const EFFECT_FS_MKDIR_PROTOCOL: &str = "effect.mutsuki.fs.mkdir";
pub const EFFECT_FS_LIST_PROTOCOL: &str = "effect.mutsuki.fs.list";
pub const EFFECT_FS_STAT_PROTOCOL: &str = "effect.mutsuki.fs.stat";
pub const EFFECT_FS_EXISTS_PROTOCOL: &str = "effect.mutsuki.fs.exists";

const FS_PROTOCOL_PAIRS: &[ProtocolPair] = &[
    ProtocolPair {
        public: FS_READ_PROTOCOL,
        effect: EFFECT_FS_READ_PROTOCOL,
        queued_event_kind: "mutsuki.effect.fs.queued",
    },
    ProtocolPair {
        public: FS_WRITE_PROTOCOL,
        effect: EFFECT_FS_WRITE_PROTOCOL,
        queued_event_kind: "mutsuki.effect.fs.queued",
    },
    ProtocolPair {
        public: FS_APPEND_PROTOCOL,
        effect: EFFECT_FS_APPEND_PROTOCOL,
        queued_event_kind: "mutsuki.effect.fs.queued",
    },
    ProtocolPair {
        public: FS_COPY_PROTOCOL,
        effect: EFFECT_FS_COPY_PROTOCOL,
        queued_event_kind: "mutsuki.effect.fs.queued",
    },
    ProtocolPair {
        public: FS_MOVE_PROTOCOL,
        effect: EFFECT_FS_MOVE_PROTOCOL,
        queued_event_kind: "mutsuki.effect.fs.queued",
    },
    ProtocolPair {
        public: FS_REMOVE_PROTOCOL,
        effect: EFFECT_FS_REMOVE_PROTOCOL,
        queued_event_kind: "mutsuki.effect.fs.queued",
    },
    ProtocolPair {
        public: FS_MKDIR_PROTOCOL,
        effect: EFFECT_FS_MKDIR_PROTOCOL,
        queued_event_kind: "mutsuki.effect.fs.queued",
    },
    ProtocolPair {
        public: FS_LIST_PROTOCOL,
        effect: EFFECT_FS_LIST_PROTOCOL,
        queued_event_kind: "mutsuki.effect.fs.queued",
    },
    ProtocolPair {
        public: FS_STAT_PROTOCOL,
        effect: EFFECT_FS_STAT_PROTOCOL,
        queued_event_kind: "mutsuki.effect.fs.queued",
    },
    ProtocolPair {
        public: FS_EXISTS_PROTOCOL,
        effect: EFFECT_FS_EXISTS_PROTOCOL,
        queued_event_kind: "mutsuki.effect.fs.queued",
    },
];

pub const FS_PROTOCOL_TABLE: ProtocolPairTable = ProtocolPairTable::new(FS_PROTOCOL_PAIRS);

#[derive(Clone)]
pub struct IoFsFacadeRunner {
    descriptor: RunnerDescriptor,
    observation: EffectObservation,
}

impl IoFsFacadeRunner {
    pub fn new() -> Self {
        Self::with_observation(EffectObservation::Quiet)
    }

    pub fn with_observation(observation: EffectObservation) -> Self {
        Self {
            descriptor: facade_runner_descriptor(),
            observation,
        }
    }
}

impl Default for IoFsFacadeRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for IoFsFacadeRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let observation = self.observation;
        map_work_batch_entries(&batch, |task| facade_result(task, observation))
    }
}

#[derive(Clone)]
pub struct IoFsEffectRunner {
    descriptor: RunnerDescriptor,
}

impl IoFsEffectRunner {
    pub fn new() -> Self {
        Self {
            descriptor: effect_runner_descriptor(),
        }
    }
}

impl Default for IoFsEffectRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for IoFsEffectRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, fs_result)
    }
}

pub fn loaded_plugin() -> LoadedPlugin {
    plugin_builder().build()
}

pub fn plugin() -> impl Plugin {
    plugin_builder()
}

fn plugin_builder() -> PluginBuilder {
    let mut builder = PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(IoFsFacadeRunner::new()))
        .runner(Box::new(IoFsEffectRunner::new()));
    for protocol_id in FS_PROTOCOL_TABLE.public_protocols() {
        builder = builder.protocol_handler(protocol_descriptor(protocol_id), RUNNER_ID, "io");
    }
    for protocol_id in FS_PROTOCOL_TABLE.effect_protocols() {
        builder = builder.protocol_descriptor(protocol_descriptor(protocol_id));
    }
    builder
}

fn facade_runner_descriptor() -> RunnerDescriptor {
    let mut builder = RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .purity(RunnerPurity::Pure)
        .execution_class(ExecutionClass::Orchestration)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::None,
            ..Default::default()
        })
        .metadata("standard_plugin", ScalarValue::String("io_fs".into()));
    for protocol_id in FS_PROTOCOL_TABLE.public_protocols() {
        builder = builder.accepted_protocol(protocol_id);
    }
    builder.build()
}

fn effect_runner_descriptor() -> RunnerDescriptor {
    let mut builder = RunnerDescriptorBuilder::new(EFFECT_RUNNER_ID, PLUGIN_ID)
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Io)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::External,
            max_inflight_batches: 1,
            preserve_order: true,
            ..Default::default()
        })
        .metadata("standard_plugin", ScalarValue::String("io_fs".into()))
        .metadata(
            "effect_execution",
            ScalarValue::String("blocking_io_isolated".into()),
        );
    for protocol_id in FS_PROTOCOL_TABLE.effect_protocols() {
        builder = builder.accepted_protocol(protocol_id);
    }
    builder.build()
}

fn protocol_descriptor(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({
            "type": "object",
            "required": ["path", "allowlist"],
            "properties": {
                "path": {"type": "string"},
                "to": {"type": "string"},
                "content": {"type": "string"},
                "allowlist": {"type": "array"}
            }
        }))
        .output_schema(json!({"type": "object"}))
        .error_schema(json!({"type": "object"}))
        .build()
}

fn facade_result(
    task: &Task,
    observation: EffectObservation,
) -> Result<RunnerResult, RuntimeError> {
    checked_path(task, "path")?;
    if matches!(
        task.protocol_id.as_str(),
        FS_COPY_PROTOCOL | FS_MOVE_PROTOCOL
    ) {
        checked_path(task, "to")?;
    }
    derive_effect_from_pair(task, &FS_PROTOCOL_TABLE, EFFECT_RUNNER_ID, observation).map_err(|_| {
        RuntimeError::new(
            mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
            "runtime.io_fs",
            format!("io_fs.protocol.{}", task.protocol_id),
        )
    })
}

fn public_protocol_for(protocol_id: &str) -> Option<&'static str> {
    FS_PROTOCOL_TABLE.public_for(protocol_id)
}

fn fs_result(task: &Task) -> Result<RunnerResult, RuntimeError> {
    let path = checked_path(task, "path")?;
    let payload = match task.protocol_id.as_str() {
        EFFECT_FS_READ_PROTOCOL => {
            let content = fs::read_to_string(&path)
                .map_err(|error| io_error(task, "mutsuki.fs.read_failed", error))?;
            json!({"path": path, "content": content, "size": content.len()})
        }
        EFFECT_FS_WRITE_PROTOCOL => {
            let content = string_payload(task, "content")?;
            atomic_write(&path, content)
                .map_err(|error| io_error(task, "mutsuki.fs.write_failed", error))?;
            json!({"path": path, "size": content.len()})
        }
        EFFECT_FS_APPEND_PROTOCOL => {
            let content = string_payload(task, "content")?;
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut file| std::io::Write::write_all(&mut file, content.as_bytes()))
                .map_err(|error| io_error(task, "mutsuki.fs.append_failed", error))?;
            json!({"path": path, "size": content.len()})
        }
        EFFECT_FS_COPY_PROTOCOL => {
            let to = checked_path(task, "to")?;
            let size = fs::copy(&path, &to)
                .map_err(|error| io_error(task, "mutsuki.fs.copy_failed", error))?;
            json!({"path": path, "to": to, "size": size})
        }
        EFFECT_FS_MOVE_PROTOCOL => {
            let to = checked_path(task, "to")?;
            fs::rename(&path, &to)
                .map_err(|error| io_error(task, "mutsuki.fs.move_failed", error))?;
            json!({"path": path, "to": to})
        }
        EFFECT_FS_REMOVE_PROTOCOL => {
            if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            }
            .map_err(|error| io_error(task, "mutsuki.fs.remove_failed", error))?;
            json!({"path": path})
        }
        EFFECT_FS_MKDIR_PROTOCOL => {
            fs::create_dir_all(&path)
                .map_err(|error| io_error(task, "mutsuki.fs.mkdir_failed", error))?;
            json!({"path": path})
        }
        EFFECT_FS_LIST_PROTOCOL => {
            let entries = fs::read_dir(&path)
                .map_err(|error| io_error(task, "mutsuki.fs.list_failed", error))?
                .map(|entry| {
                    entry
                        .map_err(|error| io_error(task, "mutsuki.fs.list_failed", error))
                        .map(|entry| entry.file_name().to_string_lossy().to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            json!({"path": path, "entries": entries})
        }
        EFFECT_FS_STAT_PROTOCOL => {
            let metadata = fs::metadata(&path)
                .map_err(|error| io_error(task, "mutsuki.fs.stat_failed", error))?;
            json!({
                "path": path,
                "is_file": metadata.is_file(),
                "is_dir": metadata.is_dir(),
                "size": metadata.len(),
            })
        }
        EFFECT_FS_EXISTS_PROTOCOL => json!({"path": path, "exists": path.exists()}),
        _ => {
            return Err(RuntimeError::new(
                mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
                "runtime.io_fs",
                format!("io_fs.protocol.{}", task.protocol_id),
            ));
        }
    };
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.events.push(DomainEvent {
        event_id: format!("event:{}:{}", task.protocol_id, task.task_id),
        kind: public_protocol_for(&task.protocol_id)
            .unwrap_or(&task.protocol_id)
            .to_string(),
        payload,
    });
    Ok(result)
}

fn checked_path(task: &Task, field: &str) -> Result<PathBuf, RuntimeError> {
    let path = task
        .payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| fs_error(task, "mutsuki.fs.invalid_path", format!("{field}.missing")))?;
    let path = absolute_path(Path::new(path))
        .map_err(|error| io_error(task, "mutsuki.fs.invalid_path", error))?;
    let allowlist = task
        .payload
        .get("allowlist")
        .and_then(Value::as_array)
        .ok_or_else(|| fs_error(task, "mutsuki.fs.permission_denied", "allowlist.missing"))?;
    for allowed in allowlist.iter().filter_map(Value::as_str) {
        let allowed = absolute_path(Path::new(allowed))
            .map_err(|error| io_error(task, "mutsuki.fs.permission_denied", error))?;
        if path.starts_with(&allowed) {
            return Ok(path);
        }
    }
    Err(fs_error(
        task,
        "mutsuki.fs.permission_denied",
        format!("{field}.outside_allowlist"),
    ))
}

fn absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    if path.exists() {
        return path.canonicalize();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.exists() {
        parent.canonicalize()?
    } else {
        absolute_path(parent)?
    };
    Ok(parent.join(path.file_name().unwrap_or_default()))
}

fn string_payload<'a>(task: &'a Task, field: &str) -> Result<&'a str, RuntimeError> {
    task.payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            fs_error(
                task,
                "mutsuki.fs.invalid_payload",
                format!("{field}.missing"),
            )
        })
}

fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("mutsuki-tmp");
    fs::write(&tmp, content)?;
    fs::rename(tmp, path)
}

fn fs_error(task: &Task, code: impl Into<String>, message: impl Into<String>) -> RuntimeError {
    let mut error = RuntimeError::new(code, "runtime.io_fs", message);
    error
        .evidence
        .insert("task_id".into(), ScalarValue::String(task.task_id.clone()));
    error
}

fn io_error(task: &Task, code: impl Into<String>, error: std::io::Error) -> RuntimeError {
    fs_error(task, code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, DispatchLane, OrderingRequirement, RunnerContext,
        WorkResourcePlan,
    };
    use mutsuki_runtime_core::Runner;
    use tempfile::TempDir;

    fn work_batch(runner_id: &str, task: Task) -> WorkBatch {
        WorkBatch {
            batch_id: format!("batch-{}", task.task_id),
            tick_id: "tick-1".into(),
            batch_key: runner_id.into(),
            entries: vec![BatchEntry {
                entry_id: task.task_id.clone(),
                task_id: task.task_id.clone(),
                trace_id: None,
                parent_id: None,
                payload_index: 0,
                resource_requirement_indices: Vec::new(),
                cancel_index: Some(0),
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::None,
            }],
            payload: BatchPayload::from_local_tasks(vec![task]),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    fn ctx() -> RunnerContext {
        RunnerContext::new(1, 1, "executor-1", None, "invoke-1")
    }

    #[test]
    fn loaded_plugin_declares_fs_protocols_and_runners() {
        let plugin = loaded_plugin();
        assert_eq!(plugin.manifest.plugin_id, PLUGIN_ID);
        assert_eq!(plugin.manifest.provides.runners[0].runner_id, RUNNER_ID);
        assert_eq!(
            plugin.manifest.provides.runners[1].runner_id,
            EFFECT_RUNNER_ID
        );
        assert_eq!(plugin.manifest.provides.protocols.len(), 20);
        assert_eq!(plugin.manifest.provides.handler_bindings.len(), 10);
        assert_eq!(
            plugin.manifest.provides.runners[0].purity,
            RunnerPurity::Pure
        );
        assert_eq!(
            plugin.manifest.provides.runners[1].purity,
            RunnerPurity::Effectful
        );
        assert_eq!(
            plugin.manifest.provides.runners[1]
                .batch
                .max_inflight_batches,
            1
        );
        FS_PROTOCOL_TABLE.validate_unique().unwrap();
    }

    #[test]
    fn facade_derives_effect_sharing_payload_without_queued_event() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("hello.txt");
        let task = Task::new(
            "fs-1",
            FS_READ_PROTOCOL,
            json!({
                "path": path.to_string_lossy(),
                "allowlist": [root.path().to_string_lossy()],
                "content": "x".repeat(8192),
            }),
        );
        let mut facade = IoFsFacadeRunner::new();
        let batch = work_batch(RUNNER_ID, task.clone());
        let completion = facade.run_batch(ctx(), batch).unwrap();
        let result = completion.results[0].result.as_ref().unwrap();
        assert!(result.events.is_empty());
        assert_eq!(result.tasks.len(), 1);
        assert_eq!(result.tasks[0].protocol_id, EFFECT_FS_READ_PROTOCOL);
        assert_eq!(task.payload.strong_count(), 2);
        assert_eq!(result.tasks[0].payload.strong_count(), 2);
    }

    #[test]
    fn detailed_facade_emits_static_queued_kind() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("hello.txt");
        let task = Task::new(
            "fs-2",
            FS_WRITE_PROTOCOL,
            json!({
                "path": path.to_string_lossy(),
                "allowlist": [root.path().to_string_lossy()],
                "content": "hello",
            }),
        );
        let mut facade = IoFsFacadeRunner::with_observation(EffectObservation::Detailed);
        let completion = facade
            .run_batch(ctx(), work_batch(RUNNER_ID, task))
            .unwrap();
        let result = completion.results[0].result.as_ref().unwrap();
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].kind, "mutsuki.effect.fs.queued");
    }

    #[test]
    fn effect_runner_reads_and_writes_roundtrip() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("data.txt");
        let allowlist = json!([root.path().to_string_lossy()]);
        let write = Task::new(
            "write-1",
            EFFECT_FS_WRITE_PROTOCOL,
            json!({
                "path": path.to_string_lossy(),
                "allowlist": allowlist,
                "content": "roundtrip",
            }),
        );
        let mut effect = IoFsEffectRunner::new();
        effect
            .run_batch(ctx(), work_batch(EFFECT_RUNNER_ID, write))
            .unwrap();
        let read = Task::new(
            "read-1",
            EFFECT_FS_READ_PROTOCOL,
            json!({
                "path": path.to_string_lossy(),
                "allowlist": allowlist,
            }),
        );
        let completion = effect
            .run_batch(ctx(), work_batch(EFFECT_RUNNER_ID, read))
            .unwrap();
        let result = completion.results[0].result.as_ref().unwrap();
        assert_eq!(
            result.events[0]
                .payload
                .get("content")
                .and_then(Value::as_str),
            Some("roundtrip")
        );
    }

    #[test]
    fn facade_denies_path_outside_allowlist() {
        let root = TempDir::new().unwrap();
        let task = Task::new(
            "deny-1",
            FS_READ_PROTOCOL,
            json!({
                "path": "/etc/passwd",
                "allowlist": [root.path().to_string_lossy()],
            }),
        );
        let mut facade = IoFsFacadeRunner::new();
        let completion = facade
            .run_batch(ctx(), work_batch(RUNNER_ID, task))
            .unwrap();
        assert!(completion.results[0].error.is_some());
    }
}
