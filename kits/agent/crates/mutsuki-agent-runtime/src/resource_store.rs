use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentError, ResourceRef, SessionDelta, SessionSnapshotRef, SessionVersion,
};
use mutsuki_runtime_contracts::{
    LeaseToken, ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Default)]
pub struct AgentResourceStore {
    inner: Arc<Mutex<ResourceStoreState>>,
}

#[derive(Default)]
struct ResourceStoreState {
    next_resource: u64,
    next_lease: u64,
    resources: BTreeMap<String, StoredResource>,
    leases: BTreeMap<String, StoredLease>,
}

struct StoredResource {
    reference: ResourceRef,
    bytes: Arc<[u8]>,
}

struct StoredLease {
    ref_id: String,
    expires_at_step: Option<u64>,
}

impl AgentResourceStore {
    pub fn put_json(
        &self,
        owner: &str,
        kind: &str,
        schema: &str,
        version: u64,
        value: &Value,
    ) -> Result<ResourceRef, AgentError> {
        if owner.trim().is_empty() || kind.trim().is_empty() || schema.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "resource owner, kind and schema are required",
            ));
        }
        let bytes = serde_json::to_vec(value)
            .map_err(|error| AgentError::invalid_input(error.to_string()))?;
        let content_hash = format!("{:x}", Sha256::digest(&bytes));
        let mut state = self.inner.lock().expect("agent resource store mutex");
        state.next_resource = state.next_resource.saturating_add(1);
        let slot_id = format!("{owner}:{}", state.next_resource);
        let ref_id = format!("{kind}:{slot_id}:{version}");
        let reference = ResourceRef {
            ref_id: ref_id.clone(),
            resource_id: ResourceId {
                kind_id: kind.into(),
                slot_id,
                generation: 1,
                version,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "mutsuki.agent.runtime.resources".into(),
            resource_kind: kind.into(),
            schema: schema.into(),
            version,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: Some(bytes.len() as u64),
            content_hash: Some(content_hash),
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        };
        state.resources.insert(
            ref_id,
            StoredResource {
                reference: reference.clone(),
                bytes: Arc::from(bytes),
            },
        );
        Ok(reference)
    }

    pub fn read_json(&self, reference: &ResourceRef) -> Result<Value, AgentError> {
        let state = self.inner.lock().expect("agent resource store mutex");
        let stored = state
            .resources
            .get(&reference.ref_id)
            .ok_or_else(|| AgentError::not_found("agent resource was not found"))?;
        if stored.reference.resource_id != reference.resource_id
            || stored.reference.content_hash != reference.content_hash
        {
            return Err(AgentError::new(
                "agent.resource.identity_mismatch",
                "resource identity or content hash does not match the immutable value",
            ));
        }
        serde_json::from_slice(&stored.bytes)
            .map_err(|error| AgentError::invalid_input(error.to_string()))
    }

    pub fn lease(
        &self,
        reference: &ResourceRef,
        owner: impl Into<String>,
        expires_at_step: Option<u64>,
    ) -> Result<AgentResourceLease, AgentError> {
        let owner = owner.into();
        if owner.trim().is_empty() {
            return Err(AgentError::invalid_input(
                "resource lease owner is required",
            ));
        }
        let mut state = self.inner.lock().expect("agent resource store mutex");
        if !state.resources.contains_key(&reference.ref_id) {
            return Err(AgentError::not_found("agent resource was not found"));
        }
        state.next_lease = state.next_lease.saturating_add(1);
        let token_id = format!("agent-resource-lease:{}", state.next_lease);
        let token = LeaseToken {
            token_id: token_id.clone(),
            ref_id: reference.ref_id.clone(),
            owner,
            mode: "read".into(),
            expires_at_step,
            generation: reference.generation,
        };
        state.leases.insert(
            token_id.clone(),
            StoredLease {
                ref_id: reference.ref_id.clone(),
                expires_at_step,
            },
        );
        let mut leased = reference.clone();
        leased.lifetime = expires_at_step.map_or(
            ResourceLifetime::BorrowedUntilTaskEnd,
            ResourceLifetime::LeaseUntil,
        );
        leased.lease = Some(token);
        Ok(AgentResourceLease {
            store: self.clone(),
            reference: leased,
            token_id: Some(token_id),
        })
    }

    pub fn active_lease_count(&self) -> usize {
        self.inner
            .lock()
            .expect("agent resource store mutex")
            .leases
            .len()
    }

    pub fn active_lease_count_for(&self, reference: &ResourceRef) -> usize {
        self.inner
            .lock()
            .expect("agent resource store mutex")
            .leases
            .values()
            .filter(|lease| lease.ref_id == reference.ref_id)
            .count()
    }

    pub fn sweep_expired(&self, current_step: u64) -> usize {
        let mut state = self.inner.lock().expect("agent resource store mutex");
        let expired = state
            .leases
            .iter()
            .filter(|(_, lease)| {
                lease
                    .expires_at_step
                    .is_some_and(|step| current_step >= step)
            })
            .map(|(token, _)| token.clone())
            .collect::<Vec<_>>();
        for token in &expired {
            state.leases.remove(token);
        }
        expired.len()
    }

    fn release(&self, token_id: &str) {
        self.inner
            .lock()
            .expect("agent resource store mutex")
            .leases
            .remove(token_id);
    }
}

pub struct AgentResourceLease {
    store: AgentResourceStore,
    reference: ResourceRef,
    token_id: Option<String>,
}

impl AgentResourceLease {
    pub fn reference(&self) -> &ResourceRef {
        &self.reference
    }

    pub fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if let Some(token_id) = self.token_id.take() {
            self.store.release(&token_id);
        }
    }
}

impl Drop for AgentResourceLease {
    fn drop(&mut self) {
        self.release_inner();
    }
}

#[derive(Clone)]
pub struct AgentSessionSnapshots {
    resources: AgentResourceStore,
}

impl AgentSessionSnapshots {
    pub fn new(resources: AgentResourceStore) -> Self {
        Self { resources }
    }

    pub fn create(
        &self,
        session_id: &str,
        value: &Value,
    ) -> Result<SessionSnapshotRef, AgentError> {
        let version = SessionVersion(1);
        let snapshot = self.resources.put_json(
            session_id,
            "mutsuki.agent.session.snapshot",
            "mutsuki.agent.session.snapshot@1",
            version.0,
            value,
        )?;
        Ok(SessionSnapshotRef {
            session_id: session_id.into(),
            version,
            snapshot,
            base: None,
            deltas: Vec::new(),
        })
    }

    pub fn advance(
        &self,
        current: &SessionSnapshotRef,
        expected_version: SessionVersion,
        delta: &Value,
    ) -> Result<(SessionSnapshotRef, SessionDelta), AgentError> {
        if current.version != expected_version {
            return Err(AgentError::new(
                "agent.session.version_conflict",
                "session snapshot version changed before delta commit",
            ));
        }
        let next_version = SessionVersion(expected_version.0.saturating_add(1));
        let delta_ref = self.resources.put_json(
            &current.session_id,
            "mutsuki.agent.session.delta",
            "mutsuki.agent.session.delta@1",
            next_version.0,
            delta,
        )?;
        let base = current
            .base
            .clone()
            .unwrap_or_else(|| current.snapshot.clone());
        let mut deltas = current.deltas.clone();
        deltas.push(delta_ref.clone());
        let chain = json!({
            "base_ref": base.ref_id,
            "delta_refs": deltas.iter().map(|item| item.ref_id.as_str()).collect::<Vec<_>>(),
        });
        let snapshot = self.resources.put_json(
            &current.session_id,
            "mutsuki.agent.session.snapshot-chain",
            "mutsuki.agent.session.snapshot-chain@1",
            next_version.0,
            &chain,
        )?;
        Ok((
            SessionSnapshotRef {
                session_id: current.session_id.clone(),
                version: next_version,
                snapshot,
                base: Some(base),
                deltas,
            },
            SessionDelta {
                session_id: current.session_id.clone(),
                expected_version,
                next_version,
                delta: delta_ref,
            },
        ))
    }

    pub fn compact(
        &self,
        current: &SessionSnapshotRef,
        materialized: &Value,
    ) -> Result<SessionSnapshotRef, AgentError> {
        let snapshot = self.resources.put_json(
            &current.session_id,
            "mutsuki.agent.session.snapshot",
            "mutsuki.agent.session.snapshot@1",
            current.version.0,
            materialized,
        )?;
        Ok(SessionSnapshotRef {
            session_id: current.session_id.clone(),
            version: current.version,
            snapshot,
            base: None,
            deltas: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn snapshot_delta_chain_is_immutable_and_compactable() {
        let resources = AgentResourceStore::default();
        let snapshots = AgentSessionSnapshots::new(resources.clone());
        let first = snapshots
            .create("session", &json!({"messages": []}))
            .unwrap();
        let (second, delta) = snapshots
            .advance(&first, SessionVersion(1), &json!({"append": ["hello"]}))
            .unwrap();

        assert_eq!(
            resources.read_json(&first.snapshot).unwrap(),
            json!({"messages": []})
        );
        assert_eq!(
            resources.read_json(&delta.delta).unwrap(),
            json!({"append": ["hello"]})
        );
        assert_eq!(second.version, SessionVersion(2));
        assert_eq!(second.deltas.len(), 1);
        assert_eq!(
            snapshots
                .advance(&second, SessionVersion(1), &json!({}))
                .unwrap_err()
                .code,
            "agent.session.version_conflict"
        );

        let compacted = snapshots
            .compact(&second, &json!({"messages": ["hello"]}))
            .unwrap();
        assert!(compacted.base.is_none());
        assert!(compacted.deltas.is_empty());
        assert_eq!(
            resources.read_json(&compacted.snapshot).unwrap(),
            json!({"messages": ["hello"]})
        );
    }

    #[test]
    fn resource_leases_release_on_finish_cancel_and_expiry() {
        let resources = AgentResourceStore::default();
        let reference = resources
            .put_json(
                "session",
                "agent.test",
                "agent.test@1",
                1,
                &json!({"ok": true}),
            )
            .unwrap();
        {
            let lease = resources.lease(&reference, "turn:finish", None).unwrap();
            assert_eq!(resources.active_lease_count(), 1);
            assert_eq!(resources.active_lease_count_for(&reference), 1);
            assert!(lease.reference().lease.is_some());
        }
        assert_eq!(resources.active_lease_count(), 0);

        resources
            .lease(&reference, "turn:cancel", None)
            .unwrap()
            .release();
        assert_eq!(resources.active_lease_count(), 0);

        let _expired = resources
            .lease(&reference, "turn:timeout", Some(7))
            .unwrap();
        assert_eq!(resources.sweep_expired(7), 1);
        assert_eq!(resources.active_lease_count(), 0);
    }
}
