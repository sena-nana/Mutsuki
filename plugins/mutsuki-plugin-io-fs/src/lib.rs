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

#[derive(Clone)]
pub struct IoFsFacadeRunner {
    descriptor: RunnerDescriptor,
}

impl IoFsFacadeRunner {
    pub fn new() -> Self {
        Self {
            descriptor: facade_runner_descriptor(),
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
        map_work_batch_entries(&batch, facade_result)
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
    for protocol_id in public_protocols() {
        builder = builder.protocol_handler(protocol_descriptor(protocol_id), RUNNER_ID, "io");
    }
    for protocol_id in effect_protocols() {
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
    for protocol_id in public_protocols() {
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
            ..Default::default()
        })
        .metadata("standard_plugin", ScalarValue::String("io_fs".into()));
    for protocol_id in effect_protocols() {
        builder = builder.accepted_protocol(protocol_id);
    }
    builder.build()
}

fn public_protocols() -> [&'static str; 10] {
    [
        FS_READ_PROTOCOL,
        FS_WRITE_PROTOCOL,
        FS_APPEND_PROTOCOL,
        FS_COPY_PROTOCOL,
        FS_MOVE_PROTOCOL,
        FS_REMOVE_PROTOCOL,
        FS_MKDIR_PROTOCOL,
        FS_LIST_PROTOCOL,
        FS_STAT_PROTOCOL,
        FS_EXISTS_PROTOCOL,
    ]
}

fn effect_protocols() -> [&'static str; 10] {
    [
        EFFECT_FS_READ_PROTOCOL,
        EFFECT_FS_WRITE_PROTOCOL,
        EFFECT_FS_APPEND_PROTOCOL,
        EFFECT_FS_COPY_PROTOCOL,
        EFFECT_FS_MOVE_PROTOCOL,
        EFFECT_FS_REMOVE_PROTOCOL,
        EFFECT_FS_MKDIR_PROTOCOL,
        EFFECT_FS_LIST_PROTOCOL,
        EFFECT_FS_STAT_PROTOCOL,
        EFFECT_FS_EXISTS_PROTOCOL,
    ]
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

fn facade_result(task: &Task) -> Result<RunnerResult, RuntimeError> {
    checked_path(task, "path")?;
    if matches!(
        task.protocol_id.as_str(),
        FS_COPY_PROTOCOL | FS_MOVE_PROTOCOL
    ) {
        checked_path(task, "to")?;
    }
    let effect_protocol = effect_protocol_for(&task.protocol_id).ok_or_else(|| {
        RuntimeError::new(
            mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
            "runtime.io_fs",
            format!("io_fs.protocol.{}", task.protocol_id),
        )
    })?;
    let mut effect_task = Task::new(
        format!("{}:effect", task.task_id),
        effect_protocol,
        task.payload.clone(),
    );
    effect_task.runner_hint = Some(EFFECT_RUNNER_ID.into());
    effect_task.trace_id = task.trace_id.clone();
    effect_task.correlation_id = task
        .correlation_id
        .clone()
        .or_else(|| Some(task.task_id.clone()));

    let mut result = RunnerResult::completed(task.task_id.clone());
    result.tasks.push(effect_task);
    result.events.push(DomainEvent {
        event_id: format!("event:{}:queued", task.task_id),
        kind: task.protocol_id.clone(),
        payload: json!({"effect_task_id": format!("{}:effect", task.task_id)}),
    });
    Ok(result)
}

fn effect_protocol_for(protocol_id: &str) -> Option<&'static str> {
    match protocol_id {
        FS_READ_PROTOCOL => Some(EFFECT_FS_READ_PROTOCOL),
        FS_WRITE_PROTOCOL => Some(EFFECT_FS_WRITE_PROTOCOL),
        FS_APPEND_PROTOCOL => Some(EFFECT_FS_APPEND_PROTOCOL),
        FS_COPY_PROTOCOL => Some(EFFECT_FS_COPY_PROTOCOL),
        FS_MOVE_PROTOCOL => Some(EFFECT_FS_MOVE_PROTOCOL),
        FS_REMOVE_PROTOCOL => Some(EFFECT_FS_REMOVE_PROTOCOL),
        FS_MKDIR_PROTOCOL => Some(EFFECT_FS_MKDIR_PROTOCOL),
        FS_LIST_PROTOCOL => Some(EFFECT_FS_LIST_PROTOCOL),
        FS_STAT_PROTOCOL => Some(EFFECT_FS_STAT_PROTOCOL),
        FS_EXISTS_PROTOCOL => Some(EFFECT_FS_EXISTS_PROTOCOL),
        _ => None,
    }
}

fn public_protocol_for(protocol_id: &str) -> Option<&'static str> {
    match protocol_id {
        EFFECT_FS_READ_PROTOCOL => Some(FS_READ_PROTOCOL),
        EFFECT_FS_WRITE_PROTOCOL => Some(FS_WRITE_PROTOCOL),
        EFFECT_FS_APPEND_PROTOCOL => Some(FS_APPEND_PROTOCOL),
        EFFECT_FS_COPY_PROTOCOL => Some(FS_COPY_PROTOCOL),
        EFFECT_FS_MOVE_PROTOCOL => Some(FS_MOVE_PROTOCOL),
        EFFECT_FS_REMOVE_PROTOCOL => Some(FS_REMOVE_PROTOCOL),
        EFFECT_FS_MKDIR_PROTOCOL => Some(FS_MKDIR_PROTOCOL),
        EFFECT_FS_LIST_PROTOCOL => Some(FS_LIST_PROTOCOL),
        EFFECT_FS_STAT_PROTOCOL => Some(FS_STAT_PROTOCOL),
        EFFECT_FS_EXISTS_PROTOCOL => Some(FS_EXISTS_PROTOCOL),
        _ => None,
    }
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
    }
}
