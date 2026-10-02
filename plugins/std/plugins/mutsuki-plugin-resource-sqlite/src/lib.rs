// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::similar_names,
    clippy::unnecessary_wraps
)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_NOT_FOUND,
    ERR_RESOURCE_UNSUPPORTED, ExportPlan, PlanReceipt, PluginManifest, ReadPlan, RefId,
    ResourceAccess, ResourceId, ResourceLifetime, ResourceProviderCompatibility,
    ResourceProviderReloadPolicy, ResourceRef, ResourceSealState, ResourceSemantic,
    ResourceTypeDescriptor, RuntimeError, ScalarValue, SnapshotDescriptor, StreamPlan, WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    LoadedPlugin, PluginBuilder, ResourceProviderExecution, ResourceProviderGateway,
};
use rusqlite::{Connection, TransactionBehavior};
use serde::Deserialize;
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.std.resource.sqlite";
pub const PROVIDER_ID: &str = "mutsuki.std.resource.sqlite";

const BLOB_KIND_ID: &str = "mutsuki.resource.sqlite.blob";
const SNAPSHOT_KIND_ID: &str = "mutsuki.resource.sqlite.snapshot";
const CAPABILITY_KIND_ID: &str = "mutsuki.resource.sqlite.capability";

/// Waited out instead of failing a plan when another connection to the same
/// database file holds the write lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Schema revision stored in `PRAGMA user_version`; bump alongside a migration.
const SCHEMA_VERSION: i64 = 3;

/// Plugin configuration accepted through the ServiceHost configured-plugin
/// document. `database_path` must point to a writable SQLite database file;
/// the parent directory is created on open.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SqliteResourceConfig {
    pub database_path: String,
    /// Absent means the database grows without bound; the deployment that owns
    /// the file decides whether its resources are disposable.
    #[serde(default)]
    pub retention: Option<SqliteRetentionConfig>,
}

/// Bounds on a resource database whose rows are disposable. Both limits are
/// optional and applied together; capability resources are never reclaimed
/// because they are long-lived handles rather than payloads.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SqliteRetentionConfig {
    #[serde(default)]
    pub max_age_seconds: Option<u64>,
    #[serde(default)]
    pub max_total_bytes: Option<u64>,
}

impl SqliteRetentionConfig {
    fn is_empty(self) -> bool {
        self.max_age_seconds.is_none() && self.max_total_bytes.is_none()
    }

    fn validate(self) -> Result<(), String> {
        if self.max_age_seconds == Some(0) {
            return Err("retention.max_age_seconds must be greater than zero".into());
        }
        if self.max_total_bytes == Some(0) {
            return Err("retention.max_total_bytes must be greater than zero".into());
        }
        Ok(())
    }
}

impl SqliteResourceConfig {
    /// # Errors
    ///
    /// Returns an error when the database path is empty or a retention bound is
    /// present but zero.
    pub fn validate(&self) -> Result<(), String> {
        if self.database_path.trim().is_empty() {
            return Err("database_path is required".into());
        }
        if let Some(retention) = self.retention {
            retention.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct SqliteResourceState {
    connection: Connection,
    reclaimed_by_create: HashMap<String, Vec<String>>,
    invalidations:
        std::cell::RefCell<Vec<mutsuki_runtime_contracts::ResourceDescriptorInvalidation>>,
}

#[derive(Debug)]
pub struct SqliteResourceProvider {
    state: Mutex<SqliteResourceState>,
    operation: Mutex<()>,
    /// Effective `journal_mode` after open (`wal`, or a non-WAL fallback such
    /// as `memory` for in-memory databases).
    journal_mode: String,
    retention: SqliteRetentionConfig,
}

impl SqliteResourceProvider {
    /// Opens (or creates) the persistent resource database at `path`.
    ///
    /// # Errors
    ///
    /// Returns a structured failure when the database cannot be opened or the
    /// schema cannot be prepared.
    pub fn open(path: &Path) -> RuntimeResult<Self> {
        Self::open_with_retention(path, SqliteRetentionConfig::default())
    }

    /// Opens the database and bounds it with `retention`. An empty retention
    /// keeps every row until a `delete` command removes it.
    ///
    /// # Errors
    ///
    /// Returns a structured failure when the database cannot be opened or the
    /// schema cannot be prepared.
    pub fn open_with_retention(
        path: &Path,
        retention: SqliteRetentionConfig,
    ) -> RuntimeResult<Self> {
        retention
            .validate()
            .map_err(|detail| unsupported("resource.sqlite.open", &detail))?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| storage_failure("resource.sqlite.open", &error.to_string()))?;
        }
        let connection = Connection::open(path)
            .map_err(|error| storage_failure("resource.sqlite.open", &error.to_string()))?;
        Self::prepare(connection, retention)
    }

    /// Opens a throwaway in-database provider; mainly for tests and default
    /// plugin construction where no product database path is configured.
    ///
    /// # Errors
    ///
    /// Returns a structured failure when the schema cannot be prepared.
    pub fn open_in_memory() -> RuntimeResult<Self> {
        let connection = Connection::open_in_memory()
            .map_err(|error| storage_failure("resource.sqlite.open", &error.to_string()))?;
        Self::prepare(connection, SqliteRetentionConfig::default())
    }

    fn prepare(connection: Connection, retention: SqliteRetentionConfig) -> RuntimeResult<Self> {
        let journal_mode = configure_connection(&connection)
            .map_err(|error| storage_failure("resource.sqlite.open", &error.to_string()))?;
        // A newer provider may have changed column meaning or invariants. Do
        // not silently operate on a schema that this binary cannot interpret;
        // `user_version >= current` would otherwise make a downgrade look
        // successful while writes corrupt the newer layout.
        let user_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| storage_failure("resource.sqlite.open", &error.to_string()))?;
        if user_version > SCHEMA_VERSION {
            return Err(storage_failure(
                "resource.sqlite.open",
                &format!(
                    "unsupported resource schema version {user_version}; maximum supported is {SCHEMA_VERSION}"
                ),
            ));
        }
        migrate_schema(&connection)
            .map_err(|error| storage_failure("resource.sqlite.open", &error.to_string()))?;
        Ok(Self {
            state: Mutex::new(SqliteResourceState {
                connection,
                reclaimed_by_create: HashMap::new(),
                invalidations: std::cell::RefCell::default(),
            }),
            operation: Mutex::new(()),
            journal_mode,
            retention,
        })
    }

    /// Effective SQLite `journal_mode` after open. Prefer `wal`; an in-memory
    /// database or a file system without shared memory keeps a fallback mode.
    #[must_use]
    pub fn journal_mode(&self) -> &str {
        &self.journal_mode
    }

    fn lock_state(&self, route: &str) -> RuntimeResult<MutexGuard<'_, SqliteResourceState>> {
        self.state
            .lock()
            .map_err(|_| storage_failure(route, "sqlite provider mutex poisoned"))
    }

    fn create_resource(
        &self,
        kind_id: &str,
        semantic: ResourceSemantic,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        const ROUTE: &str = "resource.sqlite.create";
        let mut state = self.lock_state(ROUTE)?;
        let incoming_len = match &semantic {
            ResourceSemantic::CapabilityResource => 0,
            _ => bytes.len() as u64,
        };
        if let Some(max_total_bytes) = self.retention.max_total_bytes
            && incoming_len > max_total_bytes
        {
            return Err(payload_exceeds_retention(
                ROUTE,
                incoming_len,
                max_total_bytes,
            ));
        }
        // Reclamation and insertion commit together; publish removals only after commit.
        let transaction = state
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage_failure(ROUTE, &error.to_string()))?;
        let reclaimed = if self.retention.is_empty() {
            Vec::new()
        } else {
            sweep_retention(&transaction, self.retention, incoming_len, ROUTE)?
        };
        let created_at = stored_i64(now_unix_ms(ROUTE)?, ROUTE, "created_at_unix_ms")?;
        let slot = allocate_slot(&transaction)
            .map_err(|error| storage_failure(ROUTE, &error.to_string()))?;
        let ref_id = RefId::from(format!("sqlite-resource-{slot}"));
        transaction
            .prepare_cached(
                "INSERT INTO resources
                     (ref_id, slot, kind_id, semantic, schema, version, bytes, created_at_unix_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)",
            )
            .and_then(|mut statement| {
                statement.execute(rusqlite::params![
                    ref_id.as_str(),
                    slot,
                    kind_id,
                    semantic_key(&semantic),
                    schema,
                    bytes,
                    created_at
                ])
            })
            .map_err(|error| storage_failure(ROUTE, &error.to_string()))?;
        transaction
            .commit()
            .map_err(|error| storage_failure(ROUTE, &error.to_string()))?;
        if !reclaimed.is_empty() {
            state
                .reclaimed_by_create
                .insert(ref_id.as_str().to_string(), reclaimed);
            // Vacuum after commit so a vacuum error cannot hide the insert.
            let _ = state.connection.execute_batch("PRAGMA incremental_vacuum;");
        }
        Ok(resource_ref(
            ref_id.as_str(),
            kind_id,
            semantic,
            schema,
            1,
            Some(bytes.len() as u64),
        ))
    }

    /// Reads the stored descriptor and its bytes. The blob is handed to `read`
    /// by value: a `collect` of an 8 MiB cover must not copy the row twice.
    fn with_entry<T>(
        &self,
        resource: &ResourceRef,
        route: &str,
        read: impl FnOnce(&ResourceRef, Vec<u8>) -> RuntimeResult<T>,
    ) -> RuntimeResult<T> {
        ensure_provider(resource, route)?;
        let state = self.lock_state(route)?;
        let (kind_id, semantic, schema, version, bytes) = state
            .connection
            .prepare_cached(
                "SELECT kind_id, semantic, schema, version, bytes
                 FROM resources WHERE ref_id = ?1",
            )
            .and_then(|mut statement| {
                statement.query_row([resource.ref_id.as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                })
            })
            .map_err(|error| lookup_failure(&error, route, resource.ref_id.as_str()))?;
        let current = resource_ref(
            resource.ref_id.as_str(),
            &kind_id,
            semantic_from_key(&semantic, route)?,
            &schema,
            stored_u64(version, route, "version")?,
            Some(bytes.len() as u64),
        );
        ensure_descriptor_current(resource, &current, route)?;
        read(&current, bytes)
    }

    /// Runs one capability command against an already held connection so a
    /// batch, a saga step and the capability check that precedes each of them
    /// all observe the same database state.
    fn command_locked(
        state: &SqliteResourceState,
        plan: &CommandPlan,
    ) -> RuntimeResult<PlanReceipt> {
        const ROUTE: &str = "resource.sqlite.command";
        ensure_provider(&plan.capability, ROUTE)?;
        let capability = load_descriptor(&state.connection, &plan.capability, ROUTE)?;
        ensure_descriptor_current(&plan.capability, &capability, ROUTE)?;
        if capability.semantic != ResourceSemantic::CapabilityResource {
            return Err(unsupported(ROUTE, "non_capability_resource"));
        }
        match plan.operation.as_str() {
            // The provider does not deduplicate by `idempotency_key`; the key is
            // echoed back as part of the request, not treated as a receipt id.
            "query" => Ok(PlanReceipt {
                plan_id: plan.plan_id.clone(),
                status: "commanded".into(),
                resource_ref: Some(capability),
                snapshot: None,
                descriptor_updates: Vec::new(),
                descriptor_removals: Vec::new(),
                new_version: None,
                output: json!({
                    "provider_id": PROVIDER_ID,
                    "operation": plan.operation.clone(),
                    "args": plan.args.clone(),
                    "idempotency_key": plan.idempotency_key.clone(),
                }),
            }),
            "delete" => {
                let target_ref_id = plan
                    .args
                    .get("ref_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| unsupported("resource.sqlite.command.delete", "missing ref_id"))?
                    .to_string();
                let deleted = delete_resources(
                    state,
                    "DELETE FROM resources WHERE ref_id = ?1 RETURNING ref_id",
                    [target_ref_id.as_str()],
                    "resource.sqlite.command.delete",
                )?;
                if deleted == 0 {
                    return Err(runtime_failure(
                        ERR_RESOURCE_NOT_FOUND,
                        format!("resource.sqlite.command.delete.{target_ref_id}"),
                    ));
                }
                Ok(PlanReceipt {
                    plan_id: plan.plan_id.clone(),
                    status: "deleted".into(),
                    resource_ref: Some(capability),
                    snapshot: None,
                    descriptor_updates: Vec::new(),
                    // The committed row removal is published by `execute` as a
                    // ResourceDescriptorInvalidation. Keep lifecycle removal
                    // facts out of receipts so retention and explicit delete
                    // share the same actor-owned channel.
                    descriptor_removals: Vec::new(),
                    new_version: None,
                    output: json!({ "deleted_ref_id": target_ref_id }),
                })
            }
            operation => Err(unsupported(ROUTE, operation)),
        }
    }
}

impl SqliteResourceProvider {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        match plan.operation.as_str() {
            "collect" | "get" => self.with_entry(
                &plan.resource,
                "resource.sqlite.read",
                |descriptor, bytes| {
                    if descriptor.semantic == ResourceSemantic::CapabilityResource {
                        return Err(unsupported("resource.sqlite.read", "resource_semantic"));
                    }
                    Ok(bytes)
                },
            ),
            operation => Err(unsupported("resource.sqlite.read", operation)),
        }
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        let (source_ref, source_version, bytes) = self.with_entry(
            &plan.resource,
            "resource.sqlite.snapshot",
            |descriptor, bytes| {
                ensure_materializable_semantic(descriptor, "resource.sqlite.snapshot")?;
                Ok((descriptor.clone(), descriptor.version, bytes))
            },
        )?;
        let kind_id = if kind_id.is_empty() {
            SNAPSHOT_KIND_ID
        } else {
            kind_id
        };
        let snapshot_ref =
            self.create_resource(kind_id, ResourceSemantic::VersionedSnapshot, schema, bytes)?;
        // Every snapshot is a freshly created row copied from the source
        // version just read, so it is current by construction. The provider
        // keeps no snapshot chain, which is why `is_latest` cannot mean
        // "newest of several" here.
        Ok(SnapshotDescriptor {
            snapshot_version: snapshot_ref.version,
            snapshot_ref,
            source_ref,
            source_version,
            is_stale: false,
            is_latest: true,
        })
    }

    fn open_stream_plan(plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Err(unsupported("resource.sqlite.stream", &plan.operation))
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        if plan.target != "inline_utf8" {
            return Err(unsupported("resource.sqlite.export", &plan.target));
        }
        let (resource_ref, text) = self.with_entry(
            &plan.resource,
            "resource.sqlite.export",
            |descriptor, bytes| {
                ensure_materializable_semantic(descriptor, "resource.sqlite.export")?;
                let text = String::from_utf8(bytes).map_err(|error| {
                    let mut runtime_error = RuntimeError::new(
                        ERR_RESOURCE_UNSUPPORTED,
                        "runtime.resource_provider.sqlite",
                        format!("resource.sqlite.export.{}", plan.resource.ref_id),
                    );
                    runtime_error
                        .evidence
                        .insert("detail".into(), ScalarValue::String(error.to_string()));
                    RuntimeFailure::new(runtime_error)
                })?;
                Ok((descriptor.clone(), text))
            },
        )?;
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "exported".into(),
            resource_ref: Some(resource_ref),
            snapshot: None,
            descriptor_updates: Vec::new(),
            descriptor_removals: Vec::new(),
            new_version: None,
            output: json!(text),
        })
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        const ROUTE: &str = "resource.sqlite.write";
        ensure_provider(&plan.resource, ROUTE)?;
        let mut state = self.lock_state(ROUTE)?;
        let incoming_len = bytes.len() as u64;
        if let Some(max_total_bytes) = self.retention.max_total_bytes
            && incoming_len > max_total_bytes
        {
            return Err(payload_exceeds_retention(
                ROUTE,
                incoming_len,
                max_total_bytes,
            ));
        }
        // Keep the descriptor read, capacity sweep and compare-and-swap in a
        // single write transaction. This makes a COW update obey the same
        // retention bound as create, even with another provider generation
        // sharing this SQLite file.
        let transaction = state
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage_failure(ROUTE, &error.to_string()))?;
        // Read the canonical descriptor fields along with the version.  The
        // caller's ResourceRef is an input and can carry stale or forged
        // kind/schema metadata; the receipt must never echo those fields back
        // into the Host registry after a successful write.
        let (stored_kind_id, stored_semantic, stored_schema, stored_version, stored_size) =
            transaction
                .prepare_cached(
                    "SELECT kind_id, semantic, schema, version, length(bytes)
                 FROM resources WHERE ref_id = ?1",
                )
                .and_then(|mut statement| {
                    statement.query_row([plan.resource.ref_id.as_str()], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    })
                })
                .map_err(|error| lookup_failure(&error, ROUTE, plan.resource.ref_id.as_str()))?;
        let semantic = semantic_from_key(&stored_semantic, ROUTE)?;
        let current_version = stored_u64(stored_version, ROUTE, "version")?;
        let current = resource_ref(
            plan.resource.ref_id.as_str(),
            &stored_kind_id,
            semantic.clone(),
            &stored_schema,
            current_version,
            Some(stored_u64(stored_size, ROUTE, "length")?),
        );
        ensure_descriptor_current(&plan.resource, &current, ROUTE)?;
        if plan.resource.semantic != ResourceSemantic::CowVersionedState
            || semantic != ResourceSemantic::CowVersionedState
            || plan.base_version != current_version
            || plan.patch.base_version != current_version
        {
            return Err(runtime_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                format!("{ROUTE}.{}", plan.resource.ref_id),
            ));
        }

        let reclaimed = if let Some(max_total_bytes) = self.retention.max_total_bytes {
            sweep_total_bytes_excluding(
                &transaction,
                max_total_bytes,
                incoming_len,
                plan.resource.ref_id.as_str(),
                ROUTE,
            )?
        } else {
            Vec::new()
        };

        let new_version = current_version + 1;
        let next_version = stored_i64(new_version, ROUTE, "version")?;
        // The version predicate makes the commit compare-and-swap rather than
        // last-writer-wins. The provider mutex only orders writers inside one
        // process; another process or another provider generation sharing the
        // file can still commit between the read above and this update.
        let updated = transaction
            .prepare_cached(
                "UPDATE resources SET version = ?2, bytes = ?3
                 WHERE ref_id = ?1 AND version = ?4",
            )
            .and_then(|mut statement| {
                statement.execute(rusqlite::params![
                    plan.resource.ref_id.as_str(),
                    next_version,
                    bytes,
                    stored_version
                ])
            })
            .map_err(|error| storage_failure(ROUTE, &error.to_string()))?;
        if updated == 0 {
            return Err(runtime_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                format!("{ROUTE}.{}", plan.resource.ref_id),
            ));
        }
        transaction
            .commit()
            .map_err(|error| storage_failure(ROUTE, &error.to_string()))?;
        if !reclaimed.is_empty() {
            state
                .invalidations
                .borrow_mut()
                .extend(reclaimed.into_iter().map(|ref_id| {
                    mutsuki_runtime_contracts::ResourceDescriptorInvalidation {
                        provider_id: PROVIDER_ID.into(),
                        ref_id: ref_id.into(),
                        generation: 1,
                    }
                }));
            let _ = state.connection.execute_batch("PRAGMA incremental_vacuum;");
        }
        let descriptor = resource_ref(
            plan.resource.ref_id.as_str(),
            &stored_kind_id,
            ResourceSemantic::CowVersionedState,
            &stored_schema,
            new_version,
            Some(bytes.len() as u64),
        );
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "committed".into(),
            resource_ref: Some(descriptor.clone()),
            snapshot: None,
            descriptor_updates: vec![descriptor],
            descriptor_removals: Vec::new(),
            new_version: Some(new_version),
            output: Value::Null,
        })
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        let state = self.lock_state("resource.sqlite.command")?;
        Self::command_locked(&state, plan)
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        if batch.rollback_guarantee {
            return Err(unsupported(
                "resource.sqlite.command_batch",
                "rollback_guarantee",
            ));
        }
        // No rollback is promised (`rollback_guarantee` is rejected above), but
        // the batch still runs under one guard so its commands cannot interleave
        // with an unrelated plan.
        let state = self.lock_state("resource.sqlite.command_batch")?;
        batch
            .commands
            .iter()
            .map(|command| Self::command_locked(&state, command))
            .collect()
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        let state = self.lock_state("resource.sqlite.saga")?;
        let mut receipts = Vec::new();
        for command in &saga.steps {
            match Self::command_locked(&state, command) {
                Ok(receipt) => receipts.push(receipt),
                Err(cause) => {
                    let mut runtime_error = RuntimeError::new(
                        "resource.saga_failed",
                        "runtime.resource_provider.sqlite",
                        format!("resource.sqlite.saga.{}", saga.saga_id),
                    );
                    runtime_error.cause = Some(Box::new(cause.error().clone()));
                    // A compensation that itself fails leaves the saga partly
                    // applied; surface it as evidence instead of dropping it.
                    let failures = saga
                        .compensations
                        .iter()
                        .rev()
                        .filter_map(|compensation| {
                            Self::command_locked(&state, compensation)
                                .err()
                                .map(|failure| {
                                    format!(
                                        "{}:{}",
                                        compensation.plan_id,
                                        failure.error().code.as_str()
                                    )
                                })
                        })
                        .collect::<Vec<_>>();
                    if !failures.is_empty() {
                        runtime_error.evidence.insert(
                            "compensation_failures".into(),
                            ScalarValue::String(failures.join(",")),
                        );
                    }
                    return Err(RuntimeFailure::new(runtime_error));
                }
            }
        }
        Ok(receipts)
    }
}

impl SqliteResourceProvider {
    fn create_blob_resource(&self, schema: &str, bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        self.create_resource(BLOB_KIND_ID, ResourceSemantic::FrozenValue, schema, bytes)
    }

    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.create_resource(kind_id, ResourceSemantic::CowVersionedState, schema, bytes)
    }

    fn create_capability_resource(
        &self,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        let kind_id = if kind_id.is_empty() {
            CAPABILITY_KIND_ID
        } else {
            kind_id
        };
        self.create_resource(
            kind_id,
            ResourceSemantic::CapabilityResource,
            schema,
            Vec::new(),
        )
    }
}

impl ResourceProviderGateway for SqliteResourceProvider {
    fn execute(
        &self,
        request: mutsuki_runtime_sdk::ResourceProviderRequest,
    ) -> mutsuki_runtime_sdk::ResourceProviderOutcome<mutsuki_runtime_sdk::ResourceProviderReply>
    {
        use mutsuki_runtime_sdk::{ResourceProviderReply as R, ResourceProviderRequest as Q};
        let Ok(_operation) = self.operation.lock() else {
            return mutsuki_runtime_sdk::ResourceProviderOutcome::new(Err(storage_failure(
                "resource.sqlite.execute",
                "operation mutex poisoned",
            )));
        };
        let result = match request {
            Q::CreateBlob { schema, bytes } => {
                self.create_blob_resource(&schema, bytes).map(R::Created)
            }
            Q::CreateCow {
                kind_id,
                schema,
                bytes,
            } => self
                .create_cow_state_resource(&kind_id, &schema, bytes)
                .map(R::Created),
            Q::CreateCapability { kind_id, schema } => self
                .create_capability_resource(&kind_id, &schema)
                .map(R::Created),
            Q::Collect(plan) => self.collect_read_plan(&plan).map(R::Bytes),
            Q::Snapshot {
                plan,
                kind_id,
                schema,
            } => self
                .snapshot_read_plan(&plan, &kind_id, &schema)
                .map(|value| R::Snapshot(Box::new(value))),
            Q::OpenStream(plan) => Self::open_stream_plan(&plan).map(R::Stream),
            Q::Export(plan) => self
                .execute_export_plan(&plan)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Commit { plan, bytes } => self
                .commit_write_plan(&plan, bytes)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Command(plan) => self
                .execute_command_plan(&plan)
                .map(|value| R::Receipt(Box::new(value))),
            Q::Batch(batch) => self.execute_command_batch(&batch).map(R::Receipts),
            Q::Saga(saga) => self.execute_saga_plan(&saga).map(R::Receipts),
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let created_ref_id = match &result {
            Ok(R::Created(descriptor)) => Some(descriptor.ref_id.to_string()),
            Ok(R::Snapshot(snapshot)) => Some(snapshot.snapshot_ref.ref_id.to_string()),
            _ => None,
        };
        let mut invalidations = std::mem::take(&mut *state.invalidations.borrow_mut());
        if let Some(ref_id) = created_ref_id {
            invalidations.extend(
                state
                    .reclaimed_by_create
                    .remove(&ref_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(
                        |ref_id| mutsuki_runtime_contracts::ResourceDescriptorInvalidation {
                            provider_id: PROVIDER_ID.into(),
                            ref_id: ref_id.into(),
                            generation: 1,
                        },
                    ),
            );
        }
        mutsuki_runtime_sdk::ResourceProviderOutcome {
            result,
            invalidations,
        }
    }

    fn take_reclaimed_ref_ids(&self, created_ref_id: &str) -> RuntimeResult<Vec<String>> {
        let mut state = self.lock_state("resource.sqlite.reclaim")?;
        Ok(state
            .reclaimed_by_create
            .remove(created_ref_id)
            .unwrap_or_default())
    }

    fn restore_descriptors(&self) -> RuntimeResult<Vec<ResourceRef>> {
        const ROUTE: &str = "resource.sqlite.restore";
        let state = self.lock_state(ROUTE)?;
        let rows = state
            .connection
            .prepare_cached(
                "SELECT ref_id, kind_id, semantic, schema, version, length(bytes)
                 FROM resources ORDER BY slot ASC",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|error| storage_failure(ROUTE, &error.to_string()))?;
        rows.into_iter()
            .map(|(ref_id, kind_id, semantic, schema, version, size)| {
                Ok(resource_ref(
                    &ref_id,
                    &kind_id,
                    semantic_from_key(&semantic, ROUTE)?,
                    &schema,
                    stored_u64(version, ROUTE, "version")?,
                    Some(stored_u64(size, ROUTE, "length")?),
                ))
            })
            .collect()
    }

    /// Every plan here reaches a SQLite file, so none of them belong on the
    /// Core actor thread.
    fn ordering(&self) -> mutsuki_runtime_sdk::ResourceProviderOrdering {
        mutsuki_runtime_sdk::ResourceProviderOrdering::Ordered
    }

    fn execution(&self) -> ResourceProviderExecution {
        ResourceProviderExecution::Offloaded
    }
}

/// Manifest-only identity; the ServiceHost factory supplies the file-backed
/// provider through [`loaded_plugin_with_provider`].
pub fn manifest() -> PluginManifest {
    base_builder().build().manifest
}

pub fn loaded_plugin_with_provider(provider: SqliteResourceProvider) -> LoadedPlugin {
    base_builder()
        .resource_provider_gateway(PROVIDER_ID, Arc::new(provider))
        .build()
}

fn base_builder() -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .resource_provider(PROVIDER_ID)
        .resource_type_descriptor(resource_type(
            BLOB_KIND_ID,
            ResourceSemantic::FrozenValue,
            "mutsuki.resource.sqlite.blob.v1",
            &["collect", "get", "snapshot", "export"],
        ))
        .resource_type_descriptor(resource_type(
            SNAPSHOT_KIND_ID,
            ResourceSemantic::VersionedSnapshot,
            "mutsuki.resource.sqlite.snapshot.v1",
            &["collect", "get", "export"],
        ))
        .resource_type_descriptor(resource_type(
            CAPABILITY_KIND_ID,
            ResourceSemantic::CapabilityResource,
            "mutsuki.resource.sqlite.capability.v1",
            &["query", "delete"],
        ))
}

fn resource_type(
    kind_id: &str,
    semantic: ResourceSemantic,
    schema: &str,
    operations: &[&str],
) -> ResourceTypeDescriptor {
    ResourceTypeDescriptor {
        kind_id: kind_id.into(),
        semantic,
        schema: schema.into(),
        provider_id: PROVIDER_ID.into(),
        operations: operations
            .iter()
            .map(|operation| (*operation).into())
            .collect(),
        reload_policy: ResourceProviderReloadPolicy::CompatibleWithoutLeases,
        compatibility: ResourceProviderCompatibility {
            schema_version: "1.0.0".into(),
            required_operations: operations
                .iter()
                .map(|operation| (*operation).into())
                .collect(),
            preserves_resource_type_id: true,
            accepts_older_generations: false,
            lease_drain_required: false,
        },
    }
}

fn resource_ref(
    ref_id: &str,
    kind_id: &str,
    semantic: ResourceSemantic,
    schema: &str,
    version: u64,
    size_hint: Option<u64>,
) -> ResourceRef {
    ResourceRef {
        ref_id: ref_id.into(),
        resource_id: ResourceId {
            kind_id: kind_id.into(),
            slot_id: ref_id.into(),
            generation: 1,
            version,
        },
        semantic,
        provider_id: PROVIDER_ID.into(),
        resource_kind: kind_id.into(),
        schema: schema.into(),
        version,
        generation: 1,
        access: ResourceAccess::ProviderRpc {
            provider_id: PROVIDER_ID.into(),
            method: "sqlite".into(),
        },
        size_hint,
        content_hash: None,
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

/// Loads a stored descriptor without transferring the blob: `length(bytes)`
/// supplies the size hint while the bytes stay in the database.
fn load_descriptor(
    connection: &Connection,
    resource: &ResourceRef,
    route: &str,
) -> RuntimeResult<ResourceRef> {
    let (kind_id, semantic, schema, version, size) = connection
        .prepare_cached(
            "SELECT kind_id, semantic, schema, version, length(bytes)
             FROM resources WHERE ref_id = ?1",
        )
        .and_then(|mut statement| {
            statement.query_row([resource.ref_id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
        })
        .map_err(|error| lookup_failure(&error, route, resource.ref_id.as_str()))?;
    Ok(resource_ref(
        resource.ref_id.as_str(),
        &kind_id,
        semantic_from_key(&semantic, route)?,
        &schema,
        stored_u64(version, route, "version")?,
        Some(stored_u64(size, route, "length")?),
    ))
}

/// A missing row is `resource.not_found`; anything else is a storage failure.
fn lookup_failure(error: &rusqlite::Error, route: &str, ref_id: &str) -> RuntimeFailure {
    match error {
        rusqlite::Error::QueryReturnedNoRows => {
            runtime_failure(ERR_RESOURCE_NOT_FOUND, format!("{route}.{ref_id}"))
        }
        cause => storage_failure(route, &cause.to_string()),
    }
}

/// SQLite columns are signed; a negative version or length means the row was
/// corrupted or written by something that is not this provider.
fn stored_u64(value: i64, route: &str, column: &str) -> RuntimeResult<u64> {
    u64::try_from(value)
        .map_err(|_| storage_failure(route, &format!("negative {column} column: {value}")))
}

fn stored_i64(value: u64, route: &str, column: &str) -> RuntimeResult<i64> {
    i64::try_from(value)
        .map_err(|_| storage_failure(route, &format!("{column} column overflows i64: {value}")))
}

/// Single-connection factory: busy timeout, prefer WAL, then `synchronous=NORMAL`.
/// When WAL is unavailable SQLite keeps another mode and open still succeeds, so
/// the effective mode is returned instead of being asserted.
fn configure_connection(connection: &Connection) -> rusqlite::Result<String> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(journal_mode.to_ascii_lowercase())
}

/// Applies the schema at `SCHEMA_VERSION` and records it in `PRAGMA user_version`
/// so later revisions have a migration anchor instead of relying on
/// `CREATE TABLE IF NOT EXISTS` alone.
fn migrate_schema(connection: &Connection) -> rusqlite::Result<()> {
    let user_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if user_version >= SCHEMA_VERSION {
        return Ok(());
    }
    // Only effective on a database created with this pragma in place; an
    // existing file keeps `none` and reuses freed pages instead of shrinking.
    connection.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    // v2 seeds the sequence from `MAX(slot)`, the best bound available: ids
    // deleted before the migration leave no record and can still be handed out
    // once. Every id allocated from v2 onwards is monotonic and never reused.
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS resources (
             ref_id TEXT PRIMARY KEY,
             slot INTEGER NOT NULL UNIQUE,
             kind_id TEXT NOT NULL,
             semantic TEXT NOT NULL,
             schema TEXT NOT NULL,
             version INTEGER NOT NULL,
             bytes BLOB NOT NULL
         );
         CREATE TABLE IF NOT EXISTS resource_slot_sequence (
             singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
             next_slot INTEGER NOT NULL
         );
         INSERT OR IGNORE INTO resource_slot_sequence(singleton, next_slot)
             SELECT 1, COALESCE(MAX(slot), 0) FROM resources;
         COMMIT;",
    )?;
    if user_version < 3 {
        // Rows written before v3 have no creation time. `0` keeps them outside
        // the age sweep rather than making them instantly expired.
        if !column_exists(connection, "resources", "created_at_unix_ms")? {
            connection.execute(
                "ALTER TABLE resources ADD COLUMN created_at_unix_ms INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        connection.execute_batch(
            "CREATE INDEX IF NOT EXISTS resources_created_at
                 ON resources(created_at_unix_ms);",
        )?;
    }
    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn column_exists(connection: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
    let mut columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    Ok(columns.any(|name| name.is_ok_and(|name| name == column)))
}

/// Reclaims disposable rows under `retention`. Capability resources are skipped:
/// they are handles the runtime keeps for the lifetime of the plugin, not
/// payloads. Rows without a creation time (written before schema v3) are only
/// reachable through the size bound.
///
/// `incoming_len` is reserved in the size bound so the subsequent insert cannot
/// push `SUM(length(bytes))` past `max_total_bytes`.
/// Collect identities inside the deletion transaction; publish only after commit.
fn delete_resources(
    state: &SqliteResourceState,
    sql: &str,
    params: impl rusqlite::Params,
    route: &str,
) -> RuntimeResult<usize> {
    let transaction = state
        .connection
        .unchecked_transaction()
        .map_err(|e| storage_failure(route, &e.to_string()))?;
    let ids = transaction
        .prepare(sql)
        .and_then(|mut statement| {
            statement
                .query_map(params, |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| storage_failure(route, &e.to_string()))?;
    transaction
        .commit()
        .map_err(|e| storage_failure(route, &e.to_string()))?;
    let count = ids.len();
    state
        .invalidations
        .borrow_mut()
        .extend(ids.into_iter().map(|ref_id| {
            mutsuki_runtime_contracts::ResourceDescriptorInvalidation {
                provider_id: PROVIDER_ID.into(),
                ref_id: ref_id.into(),
                generation: 1,
            }
        }));
    Ok(count)
}

fn sweep_retention(
    connection: &Connection,
    retention: SqliteRetentionConfig,
    incoming_len: u64,
    route: &str,
) -> RuntimeResult<Vec<String>> {
    let mut reclaimed = Vec::new();
    if let Some(max_age_seconds) = retention.max_age_seconds {
        reclaimed.extend(sweep_aged_rows(connection, max_age_seconds, route)?);
    }
    if let Some(max_total_bytes) = retention.max_total_bytes {
        reclaimed.extend(sweep_total_bytes(
            connection,
            max_total_bytes,
            incoming_len,
            route,
        )?);
    }
    Ok(reclaimed)
}

fn sweep_aged_rows(
    connection: &Connection,
    max_age_seconds: u64,
    route: &str,
) -> RuntimeResult<Vec<String>> {
    let cutoff = now_unix_ms(route)?.saturating_sub(max_age_seconds.saturating_mul(1_000));
    let cutoff = stored_i64(cutoff, route, "cutoff")?;
    connection
        .prepare_cached(
            "DELETE FROM resources
             WHERE semantic <> 'capability_resource'
               AND created_at_unix_ms > 0
               AND created_at_unix_ms < ?1
             RETURNING ref_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([cutoff], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| storage_failure(route, &error.to_string()))
}

/// Drops the oldest disposable rows until existing payload plus `incoming_len`
/// will fit `max_total_bytes`.
fn sweep_total_bytes(
    connection: &Connection,
    max_total_bytes: u64,
    incoming_len: u64,
    route: &str,
) -> RuntimeResult<Vec<String>> {
    sweep_total_bytes_excluding(connection, max_total_bytes, incoming_len, "", route)
}

/// Capacity sweep variant used by COW updates. The row being replaced is
/// excluded from both the total and deletion candidate set, so an update
/// cannot reclaim its own descriptor before the CAS runs.
fn sweep_total_bytes_excluding(
    connection: &Connection,
    max_total_bytes: u64,
    incoming_len: u64,
    excluded_ref_id: &str,
    route: &str,
) -> RuntimeResult<Vec<String>> {
    let target = max_total_bytes.saturating_sub(incoming_len);
    let total = connection
        .prepare_cached(
            "SELECT COALESCE(SUM(length(bytes)), 0) FROM resources
             WHERE ref_id <> ?1",
        )
        .and_then(|mut statement| {
            statement.query_row([excluded_ref_id], |row| row.get::<_, i64>(0))
        })
        .map_err(|error| storage_failure(route, &error.to_string()))?;
    let over = stored_u64(total, route, "length")?.saturating_sub(target);
    if over == 0 {
        return Ok(Vec::new());
    }
    let over = stored_i64(over, route, "length")?;
    connection
        .prepare_cached(
            "DELETE FROM resources WHERE ref_id IN (
             SELECT ref_id FROM (
                 SELECT ref_id,
                     COALESCE(SUM(length(bytes)) OVER (
                         ORDER BY slot ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING
                     ), 0) AS preceding_bytes
                 FROM resources WHERE semantic <> 'capability_resource'
                   AND ref_id <> ?2
             ) WHERE preceding_bytes < ?1
         ) RETURNING ref_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map(rusqlite::params![over, excluded_ref_id], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| storage_failure(route, &error.to_string()))
}

fn now_unix_ms(route: &str) -> RuntimeResult<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .map_err(|error| storage_failure(route, &error.to_string()))
}

/// Hands out the next resource slot from the shared sequence row. The counter
/// lives in the database rather than in provider memory, so ids stay monotonic
/// across reopen, across deletes and across provider instances that share the
/// same file (staged reload keeps two generations alive at once).
fn allocate_slot(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row(
        "UPDATE resource_slot_sequence
         SET next_slot = MAX(
             next_slot,
             COALESCE((SELECT MAX(slot) FROM resources), 0)
         ) + 1
         WHERE singleton = 1 RETURNING next_slot",
        [],
        |row| row.get(0),
    )
}

fn semantic_key(semantic: &ResourceSemantic) -> &'static str {
    match semantic {
        ResourceSemantic::FrozenValue => "frozen_value",
        ResourceSemantic::CowVersionedState => "cow_versioned_state",
        ResourceSemantic::VersionedSnapshot => "versioned_snapshot",
        ResourceSemantic::CapabilityResource => "capability_resource",
        ResourceSemantic::ReadOnlyFact => "read_only_fact",
        ResourceSemantic::StreamResource => "stream_resource",
        ResourceSemantic::TransactionResource => "transaction_resource",
    }
}

fn semantic_from_key(key: &str, route: &str) -> RuntimeResult<ResourceSemantic> {
    match key {
        "frozen_value" => Ok(ResourceSemantic::FrozenValue),
        "cow_versioned_state" => Ok(ResourceSemantic::CowVersionedState),
        "versioned_snapshot" => Ok(ResourceSemantic::VersionedSnapshot),
        "capability_resource" => Ok(ResourceSemantic::CapabilityResource),
        "read_only_fact" => Ok(ResourceSemantic::ReadOnlyFact),
        "stream_resource" => Ok(ResourceSemantic::StreamResource),
        "transaction_resource" => Ok(ResourceSemantic::TransactionResource),
        other => Err(storage_failure(
            route,
            &format!("unknown resource semantic: {other}"),
        )),
    }
}

fn ensure_provider(resource: &ResourceRef, route: &str) -> RuntimeResult<()> {
    if resource.provider_id != PROVIDER_ID {
        return Err(unsupported(route, &resource.provider_id));
    }
    Ok(())
}

/// Provider RPC plans may be constructed directly by a host or ABI caller,
/// bypassing the typed SDK semantic gates. Capability, stream and transaction
/// resources do not expose a byte snapshot/export surface, so reject those
/// plans at the provider boundary as well.
fn ensure_materializable_semantic(descriptor: &ResourceRef, route: &str) -> RuntimeResult<()> {
    if matches!(
        &descriptor.semantic,
        ResourceSemantic::CapabilityResource
            | ResourceSemantic::StreamResource
            | ResourceSemantic::TransactionResource
    ) {
        return Err(unsupported(route, "resource_semantic"));
    }
    Ok(())
}

fn ensure_descriptor_current(
    requested: &ResourceRef,
    current: &ResourceRef,
    route: &str,
) -> RuntimeResult<()> {
    if requested.generation != current.generation
        || requested.resource_id.generation != requested.generation
        || requested.version != current.version
        || requested.resource_id.version != requested.version
    {
        return Err(runtime_failure(
            ERR_RESOURCE_GENERATION_MISMATCH,
            format!("{route}.{}", requested.ref_id),
        ));
    }
    Ok(())
}

fn payload_exceeds_retention(
    route: &str,
    incoming_len: u64,
    max_total_bytes: u64,
) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        ERR_RESOURCE_UNSUPPORTED,
        "runtime.resource_provider.sqlite",
        format!("{route}.payload_exceeds_max_total_bytes"),
    );
    error.evidence.insert(
        "detail".into(),
        ScalarValue::String("payload exceeds retention.max_total_bytes".into()),
    );
    error.evidence.insert(
        "incoming_bytes".into(),
        ScalarValue::String(incoming_len.to_string()),
    );
    error.evidence.insert(
        "max_total_bytes".into(),
        ScalarValue::String(max_total_bytes.to_string()),
    );
    RuntimeFailure::new(error)
}

fn unsupported(route: &str, detail: &str) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        ERR_RESOURCE_UNSUPPORTED,
        "runtime.resource_provider.sqlite",
        route,
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.into()));
    RuntimeFailure::new(error)
}

fn runtime_failure(code: &str, route: String) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(
        code,
        "runtime.resource_provider.sqlite",
        route,
    ))
}

fn storage_failure(route: &str, detail: &str) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        "resource.storage_failed",
        "runtime.resource_provider.sqlite",
        route,
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.into()));
    RuntimeFailure::new(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::PatchDescriptor;

    #[test]
    fn blob_collect_and_inline_utf8_export_work() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let blob = provider
            .create_blob_resource("text.v1", b"hello".to_vec())
            .unwrap();
        let read = ReadPlan {
            plan_id: "read:1".into(),
            resource: blob.clone(),
            operation: "collect".into(),
            args: Value::Null,
        };
        assert_eq!(provider.collect_read_plan(&read).unwrap(), b"hello");

        let export = ExportPlan {
            plan_id: "export:1".into(),
            resource: blob,
            target: "inline_utf8".into(),
            args: Value::Null,
        };
        assert_eq!(
            provider.execute_export_plan(&export).unwrap().output,
            json!("hello")
        );
    }

    #[test]
    fn cow_commit_updates_version_and_rejects_stale_plans() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let state = provider
            .create_cow_state_resource("text_buffer", "text.state.v1", b"old".to_vec())
            .unwrap();
        let write = write_plan("write:1", state);
        let receipt = provider.commit_write_plan(&write, b"new".to_vec()).unwrap();
        assert_eq!(receipt.new_version, Some(2));

        let stale = provider
            .commit_write_plan(&write, b"stale".to_vec())
            .unwrap_err();
        assert_eq!(stale.error().code, ERR_RESOURCE_GENERATION_MISMATCH);
    }

    #[test]
    fn cow_commit_receipt_keeps_canonical_stored_descriptor_metadata() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let state = provider
            .create_cow_state_resource("stored_kind", "stored.schema.v1", b"old".to_vec())
            .unwrap();
        let mut write = write_plan("write:canonical", state);
        // Plans are caller input. A stale or forged kind/schema must not be
        // copied into the descriptor synchronized back into the Host hub.
        write.resource.resource_kind = "forged_kind".into();
        write.resource.resource_id.kind_id = "forged_kind".into();
        write.resource.schema = "forged.schema.v1".into();

        let receipt = provider.commit_write_plan(&write, b"new".to_vec()).unwrap();
        let descriptor = receipt.resource_ref.expect("committed descriptor");
        assert_eq!(descriptor.resource_kind, "stored_kind");
        assert_eq!(descriptor.resource_id.kind_id, "stored_kind");
        assert_eq!(descriptor.schema, "stored.schema.v1");
        assert_eq!(descriptor.version, 2);
    }

    #[test]
    fn cow_commit_rejects_payload_over_capacity_without_changing_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let provider = SqliteResourceProvider::open_with_retention(
            &dir.path().join("resources.db"),
            SqliteRetentionConfig {
                max_age_seconds: None,
                max_total_bytes: Some(16),
            },
        )
        .unwrap();
        let state = provider
            .create_cow_state_resource("stored_kind", "stored.schema.v1", b"old".to_vec())
            .unwrap();
        let error = provider
            .commit_write_plan(&write_plan("write:oversized", state.clone()), vec![1; 17])
            .unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_UNSUPPORTED);
        assert_eq!(
            provider
                .collect_read_plan(&ReadPlan {
                    plan_id: "read:unchanged".into(),
                    resource: state,
                    operation: "collect".into(),
                    args: Value::Null,
                })
                .unwrap(),
            b"old"
        );
    }

    #[test]
    fn cow_commit_capacity_sweep_publishes_reclaimed_ids() {
        let dir = tempfile::tempdir().unwrap();
        let provider = SqliteResourceProvider::open_with_retention(
            &dir.path().join("resources.db"),
            SqliteRetentionConfig {
                max_age_seconds: None,
                max_total_bytes: Some(8),
            },
        )
        .unwrap();
        let old = provider
            .create_blob_resource("stored_kind", vec![1; 6])
            .unwrap();
        let state = provider
            .create_cow_state_resource("stored_kind", "stored.schema.v1", vec![2])
            .unwrap();
        let outcome = provider.execute(mutsuki_runtime_sdk::ResourceProviderRequest::Commit {
            plan: Box::new(write_plan("write:sweep", state.clone())),
            bytes: vec![3; 4],
        });
        assert!(outcome.result.is_ok());
        assert_eq!(outcome.invalidations.len(), 1);
        assert_eq!(outcome.invalidations[0].ref_id, old.ref_id);
        assert_eq!(
            provider
                .collect_read_plan(&ReadPlan {
                    plan_id: "read:state".into(),
                    resource: ResourceRef {
                        version: 2,
                        resource_id: ResourceId {
                            version: 2,
                            ..state.resource_id.clone()
                        },
                        ..state
                    },
                    operation: "collect".into(),
                    args: Value::Null,
                })
                .unwrap(),
            vec![3; 4]
        );
        assert_eq!(
            provider
                .collect_read_plan(&ReadPlan {
                    plan_id: "read:old".into(),
                    resource: old,
                    operation: "collect".into(),
                    args: Value::Null,
                })
                .unwrap_err()
                .error()
                .code,
            ERR_RESOURCE_NOT_FOUND
        );
    }

    #[test]
    fn snapshot_returns_usable_snapshot_descriptor() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let blob = provider
            .create_blob_resource("text.v1", b"hello".to_vec())
            .unwrap();
        let read = ReadPlan {
            plan_id: "snapshot:1".into(),
            resource: blob,
            operation: "collect".into(),
            args: Value::Null,
        };
        let snapshot = provider
            .snapshot_read_plan(&read, "text_snapshot", "text.snapshot.v1")
            .unwrap();
        assert_eq!(
            snapshot.snapshot_ref.semantic,
            ResourceSemantic::VersionedSnapshot
        );
        assert_eq!(snapshot.snapshot_version, snapshot.snapshot_ref.version);
        assert_eq!(snapshot.source_version, 1);
        assert!(!snapshot.is_stale);
        assert!(snapshot.is_latest);
        assert_eq!(snapshot.snapshot_ref.seal_state, ResourceSealState::Sealed);
        let snapshot_read = ReadPlan {
            plan_id: "read:snapshot".into(),
            resource: snapshot.snapshot_ref,
            operation: "get".into(),
            args: Value::Null,
        };
        assert_eq!(
            provider.collect_read_plan(&snapshot_read).unwrap(),
            b"hello"
        );
    }

    #[test]
    fn capability_cannot_be_snapshotted_or_exported_through_raw_provider_plans() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let capability = provider
            .create_capability_resource("sqlite_query", "sqlite.query.v1")
            .unwrap();
        let collect = provider.collect_read_plan(&ReadPlan {
            plan_id: "read:capability".into(),
            resource: capability.clone(),
            operation: "collect".into(),
            args: Value::Null,
        });
        assert_eq!(collect.unwrap_err().error().code, ERR_RESOURCE_UNSUPPORTED);
        let snapshot = provider.snapshot_read_plan(
            &ReadPlan {
                plan_id: "snapshot:capability".into(),
                resource: capability.clone(),
                operation: "snapshot".into(),
                args: Value::Null,
            },
            "snapshot",
            "snapshot.v1",
        );
        assert_eq!(snapshot.unwrap_err().error().code, ERR_RESOURCE_UNSUPPORTED);
        let export = provider.execute_export_plan(&ExportPlan {
            plan_id: "export:capability".into(),
            resource: capability,
            target: "inline_utf8".into(),
            args: Value::Null,
        });
        assert_eq!(export.unwrap_err().error().code, ERR_RESOURCE_UNSUPPORTED);
    }

    #[test]
    fn capability_query_batch_and_saga_paths_are_deterministic() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let capability = provider
            .create_capability_resource("sqlite_query", "sqlite.query.v1")
            .unwrap();
        let command = CommandPlan {
            plan_id: "command:1".into(),
            capability: capability.clone(),
            operation: "query".into(),
            args: json!({"key": "value"}),
            idempotency_key: Some("query:1".into()),
        };
        assert_eq!(
            provider.execute_command_plan(&command).unwrap().output["provider_id"],
            PROVIDER_ID
        );
        assert_eq!(
            provider
                .execute_command_batch(&CommandBatch {
                    batch_id: "batch:1".into(),
                    commands: vec![command.clone()],
                    rollback_guarantee: false,
                })
                .unwrap()
                .len(),
            1
        );
        let rollback = provider
            .execute_command_batch(&CommandBatch {
                batch_id: "batch:rollback".into(),
                commands: vec![command.clone()],
                rollback_guarantee: true,
            })
            .unwrap_err();
        assert_eq!(rollback.error().code, ERR_RESOURCE_UNSUPPORTED);

        let mut failing = command.clone();
        failing.operation = "missing".into();
        let saga = provider.execute_saga_plan(&SagaPlan {
            saga_id: "saga:1".into(),
            steps: vec![failing],
            compensations: vec![command],
        });
        let error = saga.unwrap_err();
        assert_eq!(error.error().code, "resource.saga_failed");
        assert!(error.error().cause.is_some());
    }

    #[test]
    fn delete_command_removes_resource_and_fails_structurally_afterwards() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let capability = provider
            .create_capability_resource("sqlite_query", "sqlite.query.v1")
            .unwrap();
        let blob = provider
            .create_blob_resource("text.v1", b"doomed".to_vec())
            .unwrap();
        let delete = CommandPlan {
            plan_id: "command:delete".into(),
            capability: capability.clone(),
            operation: "delete".into(),
            args: json!({"ref_id": blob.ref_id}),
            idempotency_key: Some("delete:1".into()),
        };
        let outcome = provider.execute(mutsuki_runtime_sdk::ResourceProviderRequest::Command(
            delete.clone(),
        ));
        let receipt = match outcome.result.unwrap() {
            mutsuki_runtime_sdk::ResourceProviderReply::Receipt(receipt) => *receipt,
            other => panic!("unexpected provider reply: {other:?}"),
        };
        assert_eq!(receipt.status, "deleted");
        assert!(receipt.descriptor_removals.is_empty());
        assert_eq!(outcome.invalidations.len(), 1);
        assert_eq!(outcome.invalidations[0].provider_id, PROVIDER_ID);
        assert_eq!(outcome.invalidations[0].ref_id, blob.ref_id);
        assert_eq!(outcome.invalidations[0].generation, blob.generation);

        let read = ReadPlan {
            plan_id: "read:deleted".into(),
            resource: blob,
            operation: "collect".into(),
            args: Value::Null,
        };
        let error = provider.collect_read_plan(&read).unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_NOT_FOUND);

        let mut missing = delete.clone();
        missing.args = json!({"ref_id": "sqlite-resource-9999"});
        let error = provider.execute_command_plan(&missing).unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_NOT_FOUND);
    }

    #[test]
    fn restore_descriptors_reports_every_stored_row_without_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources.db");
        let (blob, state, capability) = {
            let provider = SqliteResourceProvider::open(&path).unwrap();
            let blob = provider
                .create_blob_resource("text.v1", b"persisted".to_vec())
                .unwrap();
            let state = provider
                .create_cow_state_resource("text_buffer", "text.state.v1", b"v1".to_vec())
                .unwrap();
            provider
                .commit_write_plan(&write_plan("write:1", state.clone()), b"v2".to_vec())
                .unwrap();
            let capability = provider
                .create_capability_resource("sqlite_query", "sqlite.query.v1")
                .unwrap();
            (blob, state, capability)
        };

        // A fresh process sees the same descriptors, at the versions the rows
        // were left at, without the Host having to have kept any of them.
        let provider = SqliteResourceProvider::open(&path).unwrap();
        let restored = provider.restore_descriptors().unwrap();
        assert_eq!(
            restored
                .iter()
                .map(|r| r.ref_id.clone())
                .collect::<Vec<_>>(),
            vec![blob.ref_id.clone(), state.ref_id.clone(), capability.ref_id]
        );
        assert_eq!(restored[0], blob);
        assert_eq!(restored[1].version, 2, "the committed version is restored");
        assert_eq!(restored[1].size_hint, Some(2));

        // A restored descriptor is immediately usable as a plan target.
        assert_eq!(
            provider
                .collect_read_plan(&ReadPlan {
                    plan_id: "read:restored".into(),
                    resource: restored[1].clone(),
                    operation: "collect".into(),
                    args: Value::Null,
                })
                .unwrap(),
            b"v2"
        );
    }

    #[test]
    fn resources_persist_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("resources.db");
        let (blob_ref, state_ref, stale_write);
        {
            let provider = SqliteResourceProvider::open(&file).unwrap();
            let blob = provider
                .create_blob_resource("text.v1", b"persisted".to_vec())
                .unwrap();
            let state = provider
                .create_cow_state_resource("text_buffer", "text.state.v1", b"v1".to_vec())
                .unwrap();
            provider
                .commit_write_plan(&write_plan("write:1", state.clone()), b"v2".to_vec())
                .unwrap();
            stale_write = write_plan("write:stale", state.clone());
            let read = ReadPlan {
                plan_id: "read:before".into(),
                resource: blob.clone(),
                operation: "collect".into(),
                args: Value::Null,
            };
            assert_eq!(provider.collect_read_plan(&read).unwrap(), b"persisted");
            blob_ref = blob;
            state_ref = state;
        }

        let provider = SqliteResourceProvider::open(&file).unwrap();
        let restored = provider.restore_descriptors().unwrap();
        let blob = ReadPlan {
            plan_id: "read:after".into(),
            resource: restored
                .iter()
                .find(|resource| resource.ref_id == blob_ref.ref_id)
                .cloned()
                .expect("blob descriptor survives reopen"),
            operation: "collect".into(),
            args: Value::Null,
        };
        assert_eq!(provider.collect_read_plan(&blob).unwrap(), b"persisted");

        let committed = ReadPlan {
            plan_id: "read:after:state".into(),
            resource: restored
                .iter()
                .find(|resource| resource.ref_id == state_ref.ref_id)
                .cloned()
                .expect("committed descriptor survives reopen"),
            operation: "collect".into(),
            args: Value::Null,
        };
        assert_eq!(provider.collect_read_plan(&committed).unwrap(), b"v2");

        let error = provider
            .commit_write_plan(&stale_write, b"stale".to_vec())
            .unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_GENERATION_MISMATCH);

        // The sequence continues past the ids already handed out before reopen
        // instead of restarting from whatever rows happen to survive.
        let created = provider
            .create_blob_resource("text.v1", b"after".to_vec())
            .unwrap();
        assert!(
            ![blob_ref.ref_id.as_str(), state_ref.ref_id.as_str()]
                .contains(&created.ref_id.as_str()),
            "reopened provider reused {}",
            created.ref_id
        );
    }

    #[test]
    fn deleted_ref_ids_are_never_handed_out_again_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources.db");
        let recycled;
        {
            let provider = SqliteResourceProvider::open(&path).unwrap();
            let capability = provider
                .create_capability_resource("sqlite_query", "sqlite.query.v1")
                .unwrap();
            let doomed = provider
                .create_blob_resource("text.v1", b"first".to_vec())
                .unwrap();
            recycled = doomed.ref_id.clone();
            provider
                .execute_command_plan(&CommandPlan {
                    plan_id: "command:delete".into(),
                    capability,
                    operation: "delete".into(),
                    args: json!({ "ref_id": doomed.ref_id }),
                    idempotency_key: None,
                })
                .unwrap();
        }

        let provider = SqliteResourceProvider::open(&path).unwrap();
        let created = provider
            .create_blob_resource("text.v1", b"second".to_vec())
            .unwrap();
        assert_ne!(
            created.ref_id, recycled,
            "a deleted ref_id was reissued and now points at different bytes"
        );
    }

    #[test]
    fn deleted_ref_ids_are_not_reused_when_database_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources.db");
        let recycled;
        {
            let provider = SqliteResourceProvider::open(&path).unwrap();
            let capability = provider
                .create_capability_resource("sqlite_query", "sqlite.query.v1")
                .unwrap();
            let capability_ref_id = capability.ref_id.clone();
            let doomed = provider
                .create_blob_resource("text.v1", b"only-row".to_vec())
                .unwrap();
            recycled = doomed.ref_id.clone();
            for ref_id in [doomed.ref_id, capability_ref_id] {
                provider
                    .execute_command_plan(&CommandPlan {
                        plan_id: format!("command:delete:{ref_id}"),
                        capability: capability.clone(),
                        operation: "delete".into(),
                        args: json!({ "ref_id": ref_id }),
                        idempotency_key: None,
                    })
                    .unwrap();
            }
            assert!(provider.restore_descriptors().unwrap().is_empty());
        }

        let provider = SqliteResourceProvider::open(&path).unwrap();
        let created = provider
            .create_blob_resource("text.v1", b"new-row".to_vec())
            .unwrap();
        assert_ne!(created.ref_id, recycled);
    }

    #[test]
    fn a_commit_from_another_provider_instance_invalidates_the_pending_plan() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources.db");
        let writer = SqliteResourceProvider::open(&path).unwrap();
        let peer = SqliteResourceProvider::open(&path).unwrap();

        let state = writer
            .create_cow_state_resource("text_buffer", "text.state.v1", b"v1".to_vec())
            .unwrap();
        let plan = write_plan("write:contended", state);

        // The peer commits first; the plan still pinned to v1 must not win.
        assert_eq!(
            peer.commit_write_plan(&plan, b"peer".to_vec())
                .unwrap()
                .new_version,
            Some(2)
        );
        let error = writer
            .commit_write_plan(&plan, b"writer".to_vec())
            .unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_GENERATION_MISMATCH);

        let read = ReadPlan {
            plan_id: "read:contended".into(),
            resource: plan.resource.clone(),
            operation: "collect".into(),
            args: Value::Null,
        };
        let mut current = read.resource.clone();
        current.version = 2;
        current.resource_id.version = 2;
        assert_eq!(
            peer.collect_read_plan(&ReadPlan {
                resource: current,
                ..read
            })
            .unwrap(),
            b"peer"
        );
    }

    #[test]
    fn saga_reports_compensation_failures_as_evidence() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let capability = provider
            .create_capability_resource("sqlite_query", "sqlite.query.v1")
            .unwrap();
        let failing_step = CommandPlan {
            plan_id: "step:missing".into(),
            capability: capability.clone(),
            operation: "missing".into(),
            args: Value::Null,
            idempotency_key: None,
        };
        let failing_compensation = CommandPlan {
            plan_id: "compensation:absent".into(),
            capability,
            operation: "delete".into(),
            args: json!({ "ref_id": "sqlite-resource-absent" }),
            idempotency_key: None,
        };
        let error = provider
            .execute_saga_plan(&SagaPlan {
                saga_id: "saga:compensation".into(),
                steps: vec![failing_step],
                compensations: vec![failing_compensation],
            })
            .unwrap_err();
        assert_eq!(error.error().code, "resource.saga_failed");
        let evidence = error
            .error()
            .evidence
            .get("compensation_failures")
            .expect("failed compensations are reported");
        assert_eq!(
            evidence,
            &ScalarValue::String(format!("compensation:absent:{ERR_RESOURCE_NOT_FOUND}"))
        );
    }

    #[test]
    fn coexisting_providers_on_one_file_allocate_distinct_ref_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources.db");
        // Staged reload keeps the outgoing generation alive while the incoming
        // one already opened the same file.
        let outgoing = SqliteResourceProvider::open(&path).unwrap();
        let incoming = SqliteResourceProvider::open(&path).unwrap();

        let first = incoming
            .create_blob_resource("text.v1", b"incoming".to_vec())
            .unwrap();
        let second = outgoing
            .create_blob_resource("text.v1", b"outgoing".to_vec())
            .unwrap();
        assert_ne!(first.ref_id, second.ref_id);

        for (provider, resource, expected) in [
            (&incoming, &first, b"incoming".as_slice()),
            (&outgoing, &second, b"outgoing".as_slice()),
        ] {
            let read = ReadPlan {
                plan_id: format!("read:{}", resource.ref_id),
                resource: resource.clone(),
                operation: "collect".into(),
                args: Value::Null,
            };
            assert_eq!(provider.collect_read_plan(&read).unwrap(), expected);
        }
    }

    #[test]
    fn file_databases_open_in_wal_and_record_the_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources.db");
        let provider = SqliteResourceProvider::open(&path).unwrap();
        assert_eq!(provider.journal_mode(), "wal");
        let recorded = |provider: &SqliteResourceProvider| -> i64 {
            provider
                .state
                .lock()
                .unwrap()
                .connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(recorded(&provider), SCHEMA_VERSION);

        let blob = provider
            .create_blob_resource("text.v1", b"kept".to_vec())
            .unwrap();
        drop(provider);

        // Reopening an already migrated database keeps the data and the version.
        let provider = SqliteResourceProvider::open(&path).unwrap();
        assert_eq!(recorded(&provider), SCHEMA_VERSION);
        let read = ReadPlan {
            plan_id: "read:after-migrate".into(),
            resource: blob,
            operation: "collect".into(),
            args: Value::Null,
        };
        assert_eq!(provider.collect_read_plan(&read).unwrap(), b"kept");
    }

    #[test]
    fn refuses_a_database_created_by_a_newer_provider() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(connection);

        let error = SqliteResourceProvider::open(&path).unwrap_err();
        let detail = error.error().evidence.get("detail").unwrap();
        assert!(matches!(
            detail,
            ScalarValue::String(detail)
                if detail.contains("unsupported resource schema version")
        ));
    }

    #[test]
    fn config_validation_requires_database_path_and_positive_retention() {
        let config = SqliteResourceConfig {
            database_path: "  ".into(),
            retention: None,
        };
        assert!(config.validate().unwrap_err().contains("database_path"));
        let config = SqliteResourceConfig {
            database_path: "/tmp/resources.db".into(),
            retention: None,
        };
        assert!(config.validate().is_ok());

        let config = SqliteResourceConfig {
            database_path: "/tmp/resources.db".into(),
            retention: Some(SqliteRetentionConfig {
                max_age_seconds: Some(0),
                max_total_bytes: None,
            }),
        };
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("retention.max_age_seconds")
        );

        // Retention is optional in the document and defaults to unbounded.
        let config: SqliteResourceConfig =
            serde_json::from_value(json!({ "database_path": "/tmp/resources.db" })).unwrap();
        assert!(config.retention.is_none());
        let config: SqliteResourceConfig = serde_json::from_value(json!({
            "database_path": "/tmp/resources.db",
            "retention": { "max_total_bytes": 1024 }
        }))
        .unwrap();
        assert_eq!(
            config.retention.unwrap().max_total_bytes,
            Some(1024),
            "retention bounds round-trip through the plugin document"
        );
    }

    #[test]
    fn direct_provider_open_rejects_zero_retention_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let error = SqliteResourceProvider::open_with_retention(
            &dir.path().join("resources.db"),
            SqliteRetentionConfig {
                max_age_seconds: Some(0),
                max_total_bytes: None,
            },
        )
        .unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_UNSUPPORTED);
        assert!(
            matches!(
                error.error().evidence.get("detail"),
                Some(ScalarValue::String(detail)) if detail.contains("max_age_seconds")
            ),
            "the invalid bound should be reported as structured provider evidence"
        );
    }

    fn stored_payload_bytes(provider: &SqliteResourceProvider) -> u64 {
        let total: i64 = provider
            .state
            .lock()
            .unwrap()
            .connection
            .query_row(
                "SELECT COALESCE(SUM(length(bytes)), 0) FROM resources",
                [],
                |row| row.get(0),
            )
            .unwrap();
        u64::try_from(total).unwrap()
    }

    #[test]
    fn retention_reclaims_aged_and_oversized_payloads_but_keeps_capabilities() {
        fn collect(
            provider: &SqliteResourceProvider,
            resource: &ResourceRef,
        ) -> RuntimeResult<Vec<u8>> {
            provider.collect_read_plan(&ReadPlan {
                plan_id: format!("read:{}", resource.ref_id),
                resource: resource.clone(),
                operation: "collect".into(),
                args: Value::Null,
            })
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources.db");

        // No age bound yet: seed rows that the size bound will have to reclaim.
        let provider = SqliteResourceProvider::open_with_retention(
            &path,
            SqliteRetentionConfig {
                max_age_seconds: None,
                max_total_bytes: Some(16),
            },
        )
        .unwrap();
        let capability = provider
            .create_capability_resource("sqlite_query", "sqlite.query.v1")
            .unwrap();
        let oldest = provider
            .create_blob_resource("text.v1", vec![b'a'; 10])
            .unwrap();
        let newest = provider
            .create_blob_resource("text.v1", vec![b'b'; 10])
            .unwrap();
        // Two 10-byte payloads cannot coexist under a 16-byte bound, so the
        // incoming write reclaims the previous blob before insert.
        assert_eq!(
            provider
                .take_reclaimed_ref_ids(newest.ref_id.as_str())
                .unwrap(),
            vec![oldest.ref_id.as_str().to_string()]
        );
        let third = provider
            .create_blob_resource("text.v1", vec![b'c'; 10])
            .unwrap();

        assert_eq!(
            collect(&provider, &oldest).unwrap_err().error().code,
            ERR_RESOURCE_NOT_FOUND
        );
        assert_eq!(
            collect(&provider, &newest).unwrap_err().error().code,
            ERR_RESOURCE_NOT_FOUND
        );
        assert!(collect(&provider, &third).is_ok());
        assert!(
            stored_payload_bytes(&provider) <= 16,
            "stored payload must stay within max_total_bytes after insert"
        );
        assert!(
            provider
                .execute_command_plan(&CommandPlan {
                    plan_id: "command:after-sweep".into(),
                    capability: capability.clone(),
                    operation: "query".into(),
                    args: Value::Null,
                    idempotency_key: None,
                })
                .is_ok(),
            "capability handles survive reclamation"
        );

        // An age bound of one second expires everything already written.
        drop(provider);
        let provider = SqliteResourceProvider::open_with_retention(
            &path,
            SqliteRetentionConfig {
                max_age_seconds: Some(1),
                max_total_bytes: None,
            },
        )
        .unwrap();
        provider
            .state
            .lock()
            .unwrap()
            .connection
            .execute("UPDATE resources SET created_at_unix_ms = 1", [])
            .unwrap();
        let fresh = provider
            .create_blob_resource("text.v1", b"fresh".to_vec())
            .unwrap();
        assert_eq!(
            collect(&provider, &third).unwrap_err().error().code,
            ERR_RESOURCE_NOT_FOUND
        );
        assert!(collect(&provider, &fresh).is_ok());
        assert!(
            provider
                .execute_command_plan(&CommandPlan {
                    plan_id: "command:after-age-sweep".into(),
                    capability,
                    operation: "query".into(),
                    args: Value::Null,
                    idempotency_key: None,
                })
                .is_ok(),
            "capability handles are exempt from the age bound too"
        );
    }

    #[test]
    fn a_payload_larger_than_max_total_bytes_fails_without_inserting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources.db");
        let provider = SqliteResourceProvider::open_with_retention(
            &path,
            SqliteRetentionConfig {
                max_age_seconds: None,
                max_total_bytes: Some(16),
            },
        )
        .unwrap();
        let capability = provider
            .create_capability_resource("sqlite_query", "sqlite.query.v1")
            .unwrap();
        let error = provider
            .create_blob_resource("text.v1", vec![b'x'; 17])
            .unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_UNSUPPORTED);
        assert_eq!(stored_payload_bytes(&provider), 0);
        assert!(
            provider
                .create_capability_resource("sqlite_query", "sqlite.query.v1")
                .is_ok(),
            "capability resources are not rejected by the size bound"
        );
        assert_eq!(capability.semantic, ResourceSemantic::CapabilityResource);
    }

    fn write_plan(plan_id: &str, resource: ResourceRef) -> WritePlan {
        WritePlan {
            plan_id: plan_id.into(),
            resource: resource.clone(),
            base_version: resource.version,
            conflict_policy: "replace".into(),
            patch: PatchDescriptor {
                patch_id: format!("patch:{plan_id}"),
                target_ref: resource.clone(),
                base_version: resource.version,
                conflict_policy: "replace".into(),
                operations: json!({"replace": true}),
            },
            returning: None,
        }
    }
}

#[cfg(test)]
mod lifecycle_tests;
