use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mutsuki_agent_contracts::{
    AgentError, AgentSessionCheckpoint, AgentSessionEvent, SessionSnapshotRef,
};
use mutsuki_agent_runtime::{AgentSessionRecovery, AgentSessionStore};
use mutsuki_runtime_contracts::Task;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct AgentDatabaseStatement {
    pub sql: String,
    pub params: Vec<Value>,
}

pub trait AgentDatabaseExecutor: Send + Sync {
    fn execute(&self, task: Task) -> Result<Value, AgentError>;
}

pub trait AgentDatabaseService: Send + Sync {
    fn query(
        &self,
        sql: &str,
        params: Vec<Value>,
        readonly: bool,
    ) -> Result<Vec<Value>, AgentError>;

    fn transaction(&self, statements: Vec<AgentDatabaseStatement>) -> Result<usize, AgentError>;
}

pub struct MutsukiDatabaseService<E> {
    executor: E,
    path: String,
    path_allowlist: Vec<String>,
    next_task_id: AtomicU64,
}

impl<E> MutsukiDatabaseService<E> {
    pub fn new(
        executor: E,
        path: impl Into<String>,
        path_allowlist: Vec<String>,
    ) -> Result<Self, AgentError> {
        let path = path.into();
        if path.trim().is_empty() || path_allowlist.is_empty() {
            return Err(AgentError::invalid_input(
                "database path and path allowlist are required",
            ));
        }
        Ok(Self {
            executor,
            path,
            path_allowlist,
            next_task_id: AtomicU64::new(0),
        })
    }
}

impl<E: AgentDatabaseExecutor> AgentDatabaseService for MutsukiDatabaseService<E> {
    fn query(
        &self,
        sql: &str,
        params: Vec<Value>,
        readonly: bool,
    ) -> Result<Vec<Value>, AgentError> {
        let result = self.execute(
            mutsuki_protocol_db::QUERY,
            json!({
                "path": self.path,
                "db_path_allowlist": self.path_allowlist,
                "sql": sql,
                "params": params,
                "readonly": readonly,
            }),
        )?;
        result
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| {
                AgentError::new(
                    "agent.database.invalid_response",
                    "Database service query response is missing rows",
                )
            })
    }

    fn transaction(&self, statements: Vec<AgentDatabaseStatement>) -> Result<usize, AgentError> {
        let result = self.execute(
            mutsuki_protocol_db::TRANSACTION,
            json!({
                "path": self.path,
                "db_path_allowlist": self.path_allowlist,
                "statements": statements
                    .into_iter()
                    .map(|statement| json!({
                        "sql": statement.sql,
                        "params": statement.params,
                    }))
                    .collect::<Vec<_>>(),
            }),
        )?;
        result
            .get("changed")
            .and_then(Value::as_u64)
            .and_then(|changed| usize::try_from(changed).ok())
            .ok_or_else(|| {
                AgentError::new(
                    "agent.database.invalid_response",
                    "Database service transaction response is missing changed",
                )
            })
    }
}

impl<E: AgentDatabaseExecutor> MutsukiDatabaseService<E> {
    fn execute(&self, protocol_id: &str, payload: Value) -> Result<Value, AgentError> {
        let id = self.next_task_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.executor.execute(Task::new(
            format!("agent-database:{id}"),
            protocol_id,
            payload,
        ))
    }
}

pub struct AgentDatabaseSessionStore<D> {
    database: Arc<D>,
}

impl<D: AgentDatabaseService> AgentDatabaseSessionStore<D> {
    pub fn new(database: Arc<D>) -> Result<Self, AgentError> {
        let store = Self { database };
        store.initialize()?;
        Ok(store)
    }

    fn initialize(&self) -> Result<(), AgentError> {
        self.database.transaction(vec![
            statement(
                "CREATE TABLE IF NOT EXISTS agent_sessions (
                    session_id TEXT PRIMARY KEY,
                    checkpoint_json TEXT,
                    checkpoint_version INTEGER NOT NULL DEFAULT 0,
                    checkpoint_sequence INTEGER NOT NULL DEFAULT 0,
                    closed INTEGER NOT NULL DEFAULT 0
                )",
                vec![],
            ),
            statement(
                "CREATE TABLE IF NOT EXISTS agent_events (
                    session_id TEXT NOT NULL,
                    sequence INTEGER NOT NULL,
                    event_json TEXT NOT NULL,
                    PRIMARY KEY(session_id, sequence)
                )",
                vec![],
            ),
            statement(
                "CREATE INDEX IF NOT EXISTS agent_events_session_sequence
                 ON agent_events(session_id, sequence)",
                vec![],
            ),
        ])?;
        Ok(())
    }

    fn checkpoint_row(
        &self,
        session_id: &str,
    ) -> Result<Option<(AgentSessionCheckpoint, u64)>, AgentError> {
        let rows = self.database.query(
            "SELECT checkpoint_json, checkpoint_sequence
             FROM agent_sessions
             WHERE session_id = ?1 AND checkpoint_json IS NOT NULL",
            vec![json!(session_id)],
            true,
        )?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let checkpoint_json = string_field(row, "checkpoint_json")?;
        let checkpoint = serde_json::from_str(checkpoint_json)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let sequence = u64_field(row, "checkpoint_sequence")?;
        Ok(Some((checkpoint, sequence)))
    }

    fn save_checkpoint_insert_only(
        &self,
        checkpoint: &AgentSessionCheckpoint,
    ) -> Result<(), AgentError> {
        let encoded = serde_json::to_string(checkpoint)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        self.database.transaction(vec![statement(
            "INSERT INTO agent_sessions(
                session_id, checkpoint_json, checkpoint_version, checkpoint_sequence, closed
             ) VALUES(?1, ?2, ?3, 0, 0)",
            vec![
                json!(checkpoint.session_id),
                json!(encoded),
                json!(checkpoint.version.0),
            ],
        )])?;
        Ok(())
    }
}

impl<D: AgentDatabaseService> AgentSessionStore for AgentDatabaseSessionStore<D> {
    fn append_event(&self, session_id: &str, event: AgentSessionEvent) -> Result<u64, AgentError> {
        validate_session_id(session_id)?;
        let encoded = serde_json::to_string(&event)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let changed = self.database.transaction(vec![
            statement(
                "INSERT INTO agent_sessions(session_id)
                 VALUES(?1)
                 ON CONFLICT(session_id) DO NOTHING",
                vec![json!(session_id)],
            ),
            statement(
                "INSERT INTO agent_events(session_id, sequence, event_json)
                 SELECT s.session_id, COALESCE(MAX(e.sequence), 0) + 1, ?2
                 FROM agent_sessions s
                 LEFT JOIN agent_events e ON e.session_id = s.session_id
                 WHERE s.session_id = ?1 AND s.closed = 0
                 GROUP BY s.session_id",
                vec![json!(session_id), json!(encoded)],
            ),
        ])?;
        if changed == 0 {
            return Err(AgentError::new(
                "agent.session.closed",
                "cannot append to a closed session",
            ));
        }
        let rows = self.database.query(
            "SELECT MAX(sequence) AS sequence
             FROM agent_events
             WHERE session_id = ?1",
            vec![json!(session_id)],
            true,
        )?;
        rows.first()
            .ok_or_else(|| AgentError::not_found("event sequence was not persisted"))
            .and_then(|row| u64_field(row, "sequence"))
    }

    fn save_checkpoint(&self, checkpoint: AgentSessionCheckpoint) -> Result<(), AgentError> {
        validate_session_id(&checkpoint.session_id)?;
        if checkpoint.snapshot.session_id != checkpoint.session_id
            || checkpoint.snapshot.version != checkpoint.version
        {
            return Err(AgentError::invalid_input(
                "checkpoint snapshot must belong to the same session and version",
            ));
        }
        let encoded = serde_json::to_string(&checkpoint)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let changed = self.database.transaction(vec![statement(
            "INSERT INTO agent_sessions(
                session_id, checkpoint_json, checkpoint_version, checkpoint_sequence, closed
             )
             VALUES(
                ?1, ?2, ?3,
                (SELECT COALESCE(MAX(sequence), 0) FROM agent_events WHERE session_id = ?1),
                0
             )
             ON CONFLICT(session_id) DO UPDATE SET
                checkpoint_json = excluded.checkpoint_json,
                checkpoint_version = excluded.checkpoint_version,
                checkpoint_sequence = excluded.checkpoint_sequence
             WHERE agent_sessions.closed = 0
               AND agent_sessions.checkpoint_version <= excluded.checkpoint_version",
            vec![
                json!(checkpoint.session_id),
                json!(encoded),
                json!(checkpoint.version.0),
            ],
        )])?;
        if changed == 0 {
            return Err(AgentError::new(
                "agent.session.version_conflict",
                "checkpoint is stale or the session is closed",
            ));
        }
        Ok(())
    }

    fn load_checkpoint(
        &self,
        session_id: &str,
    ) -> Result<Option<AgentSessionCheckpoint>, AgentError> {
        validate_session_id(session_id)?;
        Ok(self
            .checkpoint_row(session_id)?
            .map(|(checkpoint, _)| checkpoint))
    }

    fn events_after(
        &self,
        session_id: &str,
        sequence: u64,
        limit: usize,
    ) -> Result<Vec<(u64, AgentSessionEvent)>, AgentError> {
        validate_session_id(session_id)?;
        let rows = self.database.query(
            "SELECT sequence, event_json
             FROM agent_events
             WHERE session_id = ?1 AND sequence > ?2
             ORDER BY sequence ASC
             LIMIT ?3",
            vec![
                json!(session_id),
                json!(sequence),
                json!(sqlite_limit(limit)),
            ],
            true,
        )?;
        rows.into_iter()
            .map(|row| {
                let sequence = u64_field(&row, "sequence")?;
                let event = serde_json::from_str(string_field(&row, "event_json")?)
                    .map_err(|error| AgentError::invalid_input(error.to_string()))?;
                Ok((sequence, event))
            })
            .collect()
    }

    fn list_sessions(&self, limit: usize) -> Result<Vec<String>, AgentError> {
        self.database
            .query(
                "SELECT session_id
                 FROM agent_sessions
                 WHERE closed = 0
                 ORDER BY session_id ASC
                 LIMIT ?1",
                vec![json!(sqlite_limit(limit))],
                true,
            )?
            .into_iter()
            .map(|row| string_field(&row, "session_id").map(str::to_owned))
            .collect()
    }

    fn fork_session(
        &self,
        source_session_id: &str,
        target_session_id: &str,
        snapshot: SessionSnapshotRef,
    ) -> Result<(), AgentError> {
        validate_session_id(source_session_id)?;
        validate_session_id(target_session_id)?;
        if snapshot.session_id != target_session_id {
            return Err(AgentError::invalid_input(
                "fork snapshot must be rebound to the target session",
            ));
        }
        if self.load_checkpoint(target_session_id)?.is_some() {
            return Err(AgentError::new(
                "agent.session.already_exists",
                "fork target session already exists",
            ));
        }
        let source = self
            .load_checkpoint(source_session_id)?
            .ok_or_else(|| AgentError::not_found("fork source checkpoint was not found"))?;
        let checkpoint = AgentSessionCheckpoint {
            session_id: target_session_id.into(),
            profile_id: source.profile_id,
            version: snapshot.version,
            budget: source.budget,
            state: snapshot.snapshot.clone(),
            snapshot,
            pending_approvals: Vec::new(),
            plugin_generations: source.plugin_generations,
            attempts: Default::default(),
            degraded_reason: None,
        };
        self.save_checkpoint_insert_only(&checkpoint)
    }

    fn close_session(&self, session_id: &str) -> Result<(), AgentError> {
        validate_session_id(session_id)?;
        let changed = self.database.transaction(vec![statement(
            "UPDATE agent_sessions SET closed = 1 WHERE session_id = ?1 AND closed = 0",
            vec![json!(session_id)],
        )])?;
        if changed == 0 {
            return Err(AgentError::not_found("open session was not found"));
        }
        Ok(())
    }

    fn resume_session(&self, session_id: &str) -> Result<Option<AgentSessionRecovery>, AgentError> {
        validate_session_id(session_id)?;
        let Some((checkpoint, sequence)) = self.checkpoint_row(session_id)? else {
            return Ok(None);
        };
        Ok(Some(AgentSessionRecovery {
            checkpoint,
            events_after_checkpoint: self.events_after(session_id, sequence, usize::MAX)?,
        }))
    }
}

fn statement(sql: impl Into<String>, params: Vec<Value>) -> AgentDatabaseStatement {
    AgentDatabaseStatement {
        sql: sql.into(),
        params,
    }
}

fn validate_session_id(session_id: &str) -> Result<(), AgentError> {
    if session_id.trim().is_empty() {
        Err(AgentError::invalid_input("session id is required"))
    } else {
        Ok(())
    }
}

fn string_field<'a>(row: &'a Value, field: &str) -> Result<&'a str, AgentError> {
    row.get(field).and_then(Value::as_str).ok_or_else(|| {
        AgentError::new(
            "agent.database.invalid_response",
            format!("Database row is missing string field `{field}`"),
        )
    })
}

fn u64_field(row: &Value, field: &str) -> Result<u64, AgentError> {
    row.get(field).and_then(Value::as_u64).ok_or_else(|| {
        AgentError::new(
            "agent.database.invalid_response",
            format!("Database row is missing integer field `{field}`"),
        )
    })
}

fn sqlite_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use mutsuki_agent_contracts::{
        AgentAttemptCheckpoint, AgentBudget, PermissionRequest, ResourceRef, SessionVersion,
        ToolSideEffect,
    };
    use mutsuki_plugin_db_sqlite::{SqliteEffectRunner, SqliteFacadeRunner};
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, DispatchLane, EntryId, OrderingRequirement, ResourceAccess,
        ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic, RunnerContext,
        TaskLeaseId, WorkBatch, WorkResourcePlan,
    };
    use mutsuki_runtime_core::Runner;
    use tempfile::tempdir;

    use super::*;

    struct SqliteRunnerExecutor {
        facade: Mutex<SqliteFacadeRunner>,
        effect: Mutex<SqliteEffectRunner>,
    }

    impl Default for SqliteRunnerExecutor {
        fn default() -> Self {
            Self {
                facade: Mutex::new(SqliteFacadeRunner::new()),
                effect: Mutex::new(SqliteEffectRunner::new()),
            }
        }
    }

    impl AgentDatabaseExecutor for SqliteRunnerExecutor {
        fn execute(&self, task: Task) -> Result<Value, AgentError> {
            let facade = self
                .facade
                .lock()
                .unwrap()
                .run_batch(context(&task), batch(&task))
                .map_err(runtime_error)?;
            let effect_task = completion_result(&facade, "Database facade emitted no result")?
                .and_then(|result| result.tasks.first())
                .cloned()
                .ok_or_else(|| AgentError::not_found("Database facade emitted no effect task"))?;
            let effect = self
                .effect
                .lock()
                .unwrap()
                .run_batch(context(&effect_task), batch(&effect_task))
                .map_err(runtime_error)?;
            completion_result(&effect, "Database effect emitted no result")?
                .and_then(|result| result.events.first())
                .map(|event| event.payload.clone())
                .ok_or_else(|| AgentError::not_found("Database effect emitted no result event"))
        }
    }

    fn completion_result<'a>(
        completion: &'a mutsuki_runtime_contracts::CompletionBatch,
        missing: &str,
    ) -> Result<Option<&'a mutsuki_runtime_contracts::RunnerResult>, AgentError> {
        let entry = completion
            .results
            .first()
            .ok_or_else(|| AgentError::not_found(missing))?;
        if let Some(error) = &entry.error {
            return Err(AgentError::new(error.code.clone(), format!("{error:?}")));
        }
        Ok(entry.result.as_ref())
    }

    fn runtime_error(error: mutsuki_runtime_core::RuntimeFailure) -> AgentError {
        AgentError::new(error.error().code.clone(), error.to_string())
    }

    fn context(task: &Task) -> RunnerContext {
        RunnerContext::new(
            1,
            1,
            "agent-database-test",
            Vec::<TaskLeaseId>::new(),
            task.task_id.clone(),
        )
        .with_batch(format!("batch:{}", task.task_id), 1)
    }

    fn batch(task: &Task) -> WorkBatch {
        WorkBatch {
            batch_id: format!("batch:{}", task.task_id).into(),
            tick_id: "tick:agent-database".into(),
            batch_key: "database".into(),
            entries: vec![BatchEntry {
                entry_id: EntryId::from(task.task_id.as_str()),
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
            payload: BatchPayload::from_task_refs([task]),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    fn resource(session: &str, slot: &str, version: u64) -> ResourceRef {
        ResourceRef {
            ref_id: format!("{session}:{slot}:{version}").into(),
            resource_id: ResourceId {
                kind_id: "agent.session".into(),
                slot_id: slot.into(),
                generation: 1,
                version,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "test".into(),
            resource_kind: "agent.session".into(),
            schema: "agent.session@1".into(),
            version,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    fn snapshot(session: &str) -> SessionSnapshotRef {
        SessionSnapshotRef {
            session_id: session.into(),
            version: SessionVersion(1),
            snapshot: resource(session, "snapshot", 1),
            base: None,
            deltas: Vec::new(),
        }
    }

    fn checkpoint(session: &str) -> AgentSessionCheckpoint {
        AgentSessionCheckpoint {
            session_id: session.into(),
            profile_id: "coding".into(),
            version: SessionVersion(1),
            budget: AgentBudget {
                max_steps: Some(20),
                max_total_tokens: Some(10_000),
                max_cost_microunits: Some(1_000),
                deadline_unix_ms: None,
            },
            state: resource(session, "state", 1),
            snapshot: snapshot(session),
            pending_approvals: vec![PermissionRequest {
                session_id: session.into(),
                turn_id: "turn".into(),
                action_id: "write".into(),
                tool: "workspace.write".into(),
                side_effect: ToolSideEffect::WorkspaceWrite,
                summary: "write file".into(),
                version: 1,
            }],
            plugin_generations: BTreeMap::from([("lsp".into(), 7)]),
            attempts: BTreeMap::from([(
                "attempt".into(),
                AgentAttemptCheckpoint {
                    attempt_id: "attempt".into(),
                    turn_id: "turn".into(),
                    step_index: 2,
                    state: "waiting_approval".into(),
                    committed_side_effects: Vec::new(),
                },
            )]),
            degraded_reason: None,
        }
    }

    #[test]
    fn sqlite_database_service_persists_full_checkpoint_and_incremental_recovery() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("agent.sqlite");
        let database = Arc::new(
            MutsukiDatabaseService::new(
                SqliteRunnerExecutor::default(),
                path.to_string_lossy(),
                vec![temp.path().to_string_lossy().into_owned()],
            )
            .unwrap(),
        );
        let store = AgentDatabaseSessionStore::new(database.clone()).unwrap();
        store
            .append_event(
                "session",
                AgentSessionEvent::TurnStarted {
                    turn_id: "turn".into(),
                },
            )
            .unwrap();
        store.save_checkpoint(checkpoint("session")).unwrap();
        store
            .append_event(
                "session",
                AgentSessionEvent::TurnCompleted {
                    turn_id: "turn".into(),
                    result: None,
                },
            )
            .unwrap();

        let reopened = AgentDatabaseSessionStore::new(database).unwrap();
        let recovery = reopened.resume_session("session").unwrap().unwrap();
        assert_eq!(recovery.checkpoint.profile_id, "coding");
        assert_eq!(recovery.checkpoint.budget.max_steps, Some(20));
        assert_eq!(recovery.checkpoint.pending_approvals.len(), 1);
        assert_eq!(recovery.checkpoint.plugin_generations["lsp"], 7);
        assert_eq!(recovery.checkpoint.attempts["attempt"].step_index, 2);
        assert_eq!(recovery.events_after_checkpoint.len(), 1);
        assert_eq!(recovery.events_after_checkpoint[0].0, 2);

        reopened
            .fork_session("session", "fork", snapshot("fork"))
            .unwrap();
        assert_eq!(
            reopened
                .load_checkpoint("fork")
                .unwrap()
                .unwrap()
                .profile_id,
            "coding"
        );
        reopened.close_session("session").unwrap();
        assert_eq!(
            reopened
                .append_event(
                    "session",
                    AgentSessionEvent::TurnCancelled {
                        turn_id: "later".into()
                    }
                )
                .unwrap_err()
                .code,
            "agent.session.closed"
        );
    }
}
