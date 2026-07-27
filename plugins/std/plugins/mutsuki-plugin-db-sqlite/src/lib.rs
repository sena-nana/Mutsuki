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
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, OpenFlags, params_from_iter};
use serde_json::{Map, Value, json};

pub const PLUGIN_ID: &str = "mutsuki.std.db.sqlite";
pub const RUNNER_ID: &str = "mutsuki.std.db.sqlite.runner";
pub const EFFECT_RUNNER_ID: &str = "effect.mutsuki.std.db.sqlite.runner";

pub const DB_OPEN_PROTOCOL: &str = "mutsuki.db.open";
pub const DB_QUERY_PROTOCOL: &str = "mutsuki.db.query";
pub const DB_EXECUTE_PROTOCOL: &str = "mutsuki.db.execute";
pub const DB_TRANSACTION_PROTOCOL: &str = "mutsuki.db.transaction";
pub const DB_CLOSE_PROTOCOL: &str = "mutsuki.db.close";

pub const EFFECT_DB_OPEN_PROTOCOL: &str = "effect.mutsuki.db.open";
pub const EFFECT_DB_QUERY_PROTOCOL: &str = "effect.mutsuki.db.query";
pub const EFFECT_DB_EXECUTE_PROTOCOL: &str = "effect.mutsuki.db.execute";
pub const EFFECT_DB_TRANSACTION_PROTOCOL: &str = "effect.mutsuki.db.transaction";
pub const EFFECT_DB_CLOSE_PROTOCOL: &str = "effect.mutsuki.db.close";

const DB_PROTOCOL_PAIRS: &[ProtocolPair] = &[
    ProtocolPair {
        public: DB_OPEN_PROTOCOL,
        effect: EFFECT_DB_OPEN_PROTOCOL,
        queued_event_kind: "mutsuki.effect.db.queued",
    },
    ProtocolPair {
        public: DB_QUERY_PROTOCOL,
        effect: EFFECT_DB_QUERY_PROTOCOL,
        queued_event_kind: "mutsuki.effect.db.queued",
    },
    ProtocolPair {
        public: DB_EXECUTE_PROTOCOL,
        effect: EFFECT_DB_EXECUTE_PROTOCOL,
        queued_event_kind: "mutsuki.effect.db.queued",
    },
    ProtocolPair {
        public: DB_TRANSACTION_PROTOCOL,
        effect: EFFECT_DB_TRANSACTION_PROTOCOL,
        queued_event_kind: "mutsuki.effect.db.queued",
    },
    ProtocolPair {
        public: DB_CLOSE_PROTOCOL,
        effect: EFFECT_DB_CLOSE_PROTOCOL,
        queued_event_kind: "mutsuki.effect.db.queued",
    },
];

pub const DB_PROTOCOL_TABLE: ProtocolPairTable = ProtocolPairTable::new(DB_PROTOCOL_PAIRS);

#[derive(Clone)]
pub struct SqliteFacadeRunner {
    descriptor: RunnerDescriptor,
    observation: EffectObservation,
}

impl SqliteFacadeRunner {
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

impl Default for SqliteFacadeRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for SqliteFacadeRunner {
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
pub struct SqliteEffectRunner {
    descriptor: RunnerDescriptor,
}

impl SqliteEffectRunner {
    pub fn new() -> Self {
        Self {
            descriptor: effect_runner_descriptor(),
        }
    }
}

impl Default for SqliteEffectRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for SqliteEffectRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, effect_result)
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
        .runner(Box::new(SqliteFacadeRunner::new()))
        .runner(Box::new(SqliteEffectRunner::new()));
    for protocol_id in DB_PROTOCOL_TABLE.public_protocols() {
        builder = builder.protocol_handler(protocol_descriptor(protocol_id), RUNNER_ID, "db");
    }
    for protocol_id in DB_PROTOCOL_TABLE.effect_protocols() {
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
        .metadata("standard_plugin", ScalarValue::String("db_sqlite".into()));
    for protocol_id in DB_PROTOCOL_TABLE.public_protocols() {
        builder = builder.accepted_protocol(protocol_id);
    }
    builder.build()
}

fn effect_runner_descriptor() -> RunnerDescriptor {
    let mut builder = RunnerDescriptorBuilder::new(EFFECT_RUNNER_ID, PLUGIN_ID)
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Blocking)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::External,
            max_inflight_batches: 1,
            preserve_order: true,
            ..Default::default()
        })
        .metadata("standard_plugin", ScalarValue::String("db_sqlite".into()))
        .metadata(
            "effect_execution",
            ScalarValue::String("blocking_io_isolated".into()),
        );
    for protocol_id in DB_PROTOCOL_TABLE.effect_protocols() {
        builder = builder.accepted_protocol(protocol_id);
    }
    builder.build()
}

fn protocol_descriptor(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "db_path_allowlist": {"type": "array"},
                "sql": {"type": "string"},
                "params": {"type": "array"},
                "statements": {"type": "array"},
                "readonly": {"type": "boolean"}
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
    if task.protocol_id != DB_CLOSE_PROTOCOL {
        checked_path(task)?;
    }
    derive_effect_from_pair(task, &DB_PROTOCOL_TABLE, EFFECT_RUNNER_ID, observation).map_err(|_| {
        RuntimeError::new(
            mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
            "runtime.db_sqlite",
            format!("db_sqlite.protocol.{}", task.protocol_id),
        )
    })
}

fn public_protocol_for(protocol_id: &str) -> Option<&'static str> {
    DB_PROTOCOL_TABLE.public_for(protocol_id)
}

fn effect_result(task: &Task) -> Result<RunnerResult, RuntimeError> {
    let payload = match task.protocol_id.as_str() {
        EFFECT_DB_OPEN_PROTOCOL => {
            let path = checked_path(task)?;
            let _connection = open_connection(task, &path)?;
            json!({"path": path, "opened": true})
        }
        EFFECT_DB_QUERY_PROTOCOL => {
            let path = checked_path(task)?;
            let connection = open_connection(task, &path)?;
            let rows = query_rows(task, &connection)?;
            json!({"path": path, "rows": rows})
        }
        EFFECT_DB_EXECUTE_PROTOCOL => {
            let path = checked_path(task)?;
            let connection = open_connection(task, &path)?;
            let changed = execute_sql(task, &connection)?;
            json!({"path": path, "changed": changed})
        }
        EFFECT_DB_TRANSACTION_PROTOCOL => {
            let path = checked_path(task)?;
            let mut connection = open_connection(task, &path)?;
            let changed = execute_transaction(task, &mut connection)?;
            json!({"path": path, "changed": changed})
        }
        EFFECT_DB_CLOSE_PROTOCOL => json!({"closed": true}),
        _ => {
            return Err(RuntimeError::new(
                mutsuki_runtime_contracts::ERR_TASK_UNSUPPORTED,
                "runtime.db_sqlite",
                format!("db_sqlite.protocol.{}", task.protocol_id),
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

fn query_rows(task: &Task, connection: &Connection) -> Result<Vec<Value>, RuntimeError> {
    let sql = sql(task)?;
    let params = sql_params(task);
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| sqlite_error(task, "mutsuki.db.query_failed", error))?;
    let columns: Vec<_> = statement
        .column_names()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect();
    let rows = statement
        .query_map(params_from_iter(params), |row| {
            let mut object = Map::new();
            for (index, column) in columns.iter().enumerate() {
                object.insert(column.clone(), sqlite_value(row.get_ref(index)?));
            }
            Ok(Value::Object(object))
        })
        .map_err(|error| sqlite_error(task, "mutsuki.db.query_failed", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sqlite_error(task, "mutsuki.db.query_failed", error))?;
    Ok(rows)
}

fn execute_sql(task: &Task, connection: &Connection) -> Result<usize, RuntimeError> {
    connection
        .execute(sql(task)?, params_from_iter(sql_params(task)))
        .map_err(|error| sqlite_error(task, "mutsuki.db.execute_failed", error))
}

fn execute_transaction(task: &Task, connection: &mut Connection) -> Result<usize, RuntimeError> {
    let statements = task
        .payload
        .get("statements")
        .and_then(Value::as_array)
        .ok_or_else(|| db_error(task, "mutsuki.db.invalid_payload", "statements.missing"))?;
    let tx = connection
        .transaction()
        .map_err(|error| sqlite_error(task, "mutsuki.db.transaction_failed", error))?;
    let mut changed = 0;
    for statement in statements {
        let sql = statement
            .get("sql")
            .and_then(Value::as_str)
            .ok_or_else(|| db_error(task, "mutsuki.db.invalid_payload", "statement.sql.missing"))?;
        let params = statement
            .get("params")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(json_to_sql_value)
            .collect::<Vec<_>>();
        changed += tx
            .execute(sql, params_from_iter(params))
            .map_err(|error| sqlite_error(task, "mutsuki.db.transaction_failed", error))?;
    }
    tx.commit()
        .map_err(|error| sqlite_error(task, "mutsuki.db.transaction_failed", error))?;
    Ok(changed)
}

fn open_connection(task: &Task, path: &Path) -> Result<Connection, RuntimeError> {
    let flags = if task
        .payload
        .get("readonly")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    };
    Connection::open_with_flags(path, flags)
        .map_err(|error| sqlite_error(task, "mutsuki.db.open_failed", error))
}

fn checked_path(task: &Task) -> Result<PathBuf, RuntimeError> {
    let path = task
        .payload
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| db_error(task, "mutsuki.db.invalid_path", "path.missing"))?;
    let path = absolute_path(Path::new(path)).map_err(|error| {
        db_error(
            task,
            "mutsuki.db.invalid_path",
            format!("path.resolve.{error}"),
        )
    })?;
    let allowlist = task
        .payload
        .get("db_path_allowlist")
        .and_then(Value::as_array)
        .ok_or_else(|| db_error(task, "mutsuki.db.permission_denied", "allowlist.missing"))?;
    for allowed in allowlist.iter().filter_map(Value::as_str) {
        let allowed = absolute_path(Path::new(allowed)).map_err(|error| {
            db_error(
                task,
                "mutsuki.db.permission_denied",
                format!("allowlist.resolve.{error}"),
            )
        })?;
        if path.starts_with(&allowed) {
            return Ok(path);
        }
    }
    Err(db_error(
        task,
        "mutsuki.db.permission_denied",
        "path.outside_allowlist",
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

fn sql(task: &Task) -> Result<&str, RuntimeError> {
    task.payload
        .get("sql")
        .and_then(Value::as_str)
        .ok_or_else(|| db_error(task, "mutsuki.db.invalid_payload", "sql.missing"))
}

fn sql_params(task: &Task) -> Vec<SqlValue> {
    task.payload
        .get("params")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(json_to_sql_value)
        .collect()
}

fn json_to_sql_value(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(value) => SqlValue::Integer(i64::from(*value)),
        Value::Number(value) => value
            .as_i64()
            .map(SqlValue::Integer)
            .or_else(|| value.as_f64().map(SqlValue::Real))
            .unwrap_or(SqlValue::Null),
        Value::String(value) => SqlValue::Text(value.clone()),
        Value::Array(_) | Value::Object(_) => SqlValue::Text(value.to_string()),
    }
}

fn sqlite_value(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => json!(value),
        ValueRef::Real(value) => json!(value),
        ValueRef::Text(value) => json!(String::from_utf8_lossy(value).to_string()),
        ValueRef::Blob(value) => json!({"blob_size": value.len()}),
    }
}

fn db_error(task: &Task, code: impl Into<String>, message: impl Into<String>) -> RuntimeError {
    let mut error = RuntimeError::new(code, "runtime.db_sqlite", message);
    error
        .evidence
        .insert("task_id".into(), ScalarValue::String(task.task_id.clone()));
    error
}

fn sqlite_error(task: &Task, code: impl Into<String>, error: rusqlite::Error) -> RuntimeError {
    db_error(task, code, error.to_string())
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
    fn loaded_plugin_declares_sqlite_protocols_and_runners() {
        let plugin = loaded_plugin();
        assert_eq!(plugin.manifest.plugin_id, PLUGIN_ID);
        assert_eq!(plugin.manifest.provides.runners[0].runner_id, RUNNER_ID);
        assert_eq!(
            plugin.manifest.provides.runners[1].runner_id,
            EFFECT_RUNNER_ID
        );
        assert_eq!(plugin.manifest.provides.protocols.len(), 10);
        assert_eq!(plugin.manifest.provides.handler_bindings.len(), 5);
        DB_PROTOCOL_TABLE.validate_unique().unwrap();
    }

    #[test]
    fn facade_to_effect_query_roundtrip() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("demo.db");
        let allow = json!([root.path().to_string_lossy()]);
        let open = Task::new(
            "open-1",
            EFFECT_DB_OPEN_PROTOCOL,
            json!({"path": path.to_string_lossy(), "db_path_allowlist": allow}),
        );
        let mut effect = SqliteEffectRunner::new();
        effect
            .run_batch(ctx(), work_batch(EFFECT_RUNNER_ID, open))
            .unwrap();
        let execute = Task::new(
            "exec-1",
            EFFECT_DB_EXECUTE_PROTOCOL,
            json!({
                "path": path.to_string_lossy(),
                "db_path_allowlist": allow,
                "sql": "create table t(id integer); insert into t values (1);"
            }),
        );
        effect
            .run_batch(ctx(), work_batch(EFFECT_RUNNER_ID, execute))
            .unwrap();
        let facade_task = Task::new(
            "query-1",
            DB_QUERY_PROTOCOL,
            json!({
                "path": path.to_string_lossy(),
                "db_path_allowlist": allow,
                "sql": "select id from t",
                "content": "y".repeat(2048),
            }),
        );
        let mut facade = SqliteFacadeRunner::new();
        let derived = facade
            .run_batch(ctx(), work_batch(RUNNER_ID, facade_task.clone()))
            .unwrap();
        let result = derived.results[0].result.as_ref().unwrap();
        assert!(result.events.is_empty());
        assert_eq!(facade_task.payload.strong_count(), 2);
        let query = result.tasks[0].clone();
        let done = effect
            .run_batch(ctx(), work_batch(EFFECT_RUNNER_ID, query))
            .unwrap();
        assert!(done.results[0].result.is_some());
    }
}
