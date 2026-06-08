use std::collections::HashMap;

use mutsuki_runtime_contracts::{
    LeaseToken, RefDescriptor, ResourceRecord, RuntimeError, ScalarValue,
};

use crate::backend::ResourceBackend;
use crate::error::{RuntimeFailure, RuntimeResult};
use crate::id::{IdSource, SequentialIdSource};

#[derive(Clone, Debug)]
pub struct ResourceGate {
    records: HashMap<String, ResourceRecord>,
    leases: HashMap<String, LeaseToken>,
    id_source: SequentialIdSource,
}

impl Default for ResourceGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceGate {
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
            leases: HashMap::new(),
            id_source: SequentialIdSource::new(),
        }
    }

    pub fn with_id_source(id_source: SequentialIdSource) -> Self {
        Self {
            records: HashMap::new(),
            leases: HashMap::new(),
            id_source,
        }
    }

    pub fn register(&mut self, descriptor: RefDescriptor, owner: impl Into<String>) -> String {
        let ref_id = descriptor.ref_id.clone();
        self.records.insert(
            ref_id.clone(),
            ResourceRecord {
                descriptor,
                owner: owner.into(),
                lease_count: 0,
            },
        );
        ref_id
    }

    pub fn acquire(
        &mut self,
        ref_id: &str,
        requester: impl Into<String>,
    ) -> RuntimeResult<LeaseToken> {
        let record = self.records.get_mut(ref_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                "ref.not_found",
                "runtime.resource_gate",
                format!("runtime.resource.acquire.{ref_id}"),
            ))
        })?;
        record.lease_count += 1;
        let token = LeaseToken {
            token_id: self.id_source.next_id("lease"),
            ref_id: ref_id.to_string(),
            owner: requester.into(),
        };
        self.leases.insert(token.token_id.clone(), token.clone());
        Ok(token)
    }

    pub fn release(&mut self, token: &LeaseToken) -> RuntimeResult<()> {
        let stored = self.leases.get(&token.token_id).ok_or_else(|| {
            RuntimeFailure::new(RuntimeError::new(
                "ref.not_found",
                "runtime.resource_gate",
                format!("runtime.resource.release.{}", token.token_id),
            ))
        })?;
        if stored != token {
            let mut err = RuntimeError::new(
                "ref.not_found",
                "runtime.resource_gate",
                format!("runtime.resource.release.{}", token.token_id),
            );
            err.evidence.insert(
                "reason".into(),
                ScalarValue::String("lease_token_mismatch".into()),
            );
            err.evidence.insert(
                "token_id".into(),
                ScalarValue::String(token.token_id.clone()),
            );
            err.evidence.insert(
                "expected_ref_id".into(),
                ScalarValue::String(stored.ref_id.clone()),
            );
            err.evidence.insert(
                "actual_ref_id".into(),
                ScalarValue::String(token.ref_id.clone()),
            );
            err.evidence.insert(
                "expected_owner".into(),
                ScalarValue::String(stored.owner.clone()),
            );
            err.evidence.insert(
                "actual_owner".into(),
                ScalarValue::String(token.owner.clone()),
            );
            return Err(RuntimeFailure::new(err));
        }
        let removed = self
            .leases
            .remove(&token.token_id)
            .expect("lease exists after prior lookup");
        if let Some(record) = self.records.get_mut(&removed.ref_id) {
            record.lease_count = record.lease_count.saturating_sub(1);
        }
        Ok(())
    }

    pub fn list_records(&self) -> Vec<ResourceRecord> {
        self.list_records_for(None)
    }

    pub fn list_records_for(&self, owner: Option<&str>) -> Vec<ResourceRecord> {
        let mut records: Vec<ResourceRecord> = self
            .records
            .values()
            .filter(|record| owner.is_none_or(|target| record.owner == target))
            .cloned()
            .collect();
        records.sort_by(|a, b| a.descriptor.ref_id.cmp(&b.descriptor.ref_id));
        records
    }
}

impl ResourceBackend for ResourceGate {
    fn register_resource(
        &mut self,
        descriptor: RefDescriptor,
        owner: &str,
    ) -> RuntimeResult<String> {
        Ok(self.register(descriptor, owner))
    }

    fn acquire_resource(&mut self, ref_id: &str, requester: &str) -> RuntimeResult<LeaseToken> {
        self.acquire(ref_id, requester)
    }

    fn release_resource(&mut self, token: &LeaseToken) -> RuntimeResult<()> {
        self.release(token)
    }

    fn list_records(&self, owner: Option<&str>) -> Vec<ResourceRecord> {
        self.list_records_for(owner)
    }
}
