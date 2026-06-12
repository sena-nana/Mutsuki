use std::collections::{BTreeMap, HashMap};

use mutsuki_runtime_contracts::{
    ERR_CAPABILITY_EXHAUSTED, LeaseToken, RefDescriptor, ResourceRecord, RuntimeError,
    RuntimeEventKind, ScalarValue,
};

use crate::backend::ResourceBackend;
use crate::error::{RuntimeFailure, RuntimeResult};
use crate::event::EventDraft;
use crate::id::{IdSource, SequentialIdSource};

#[derive(Clone, Debug)]
pub struct ResourceGate {
    records: HashMap<String, ResourceRecord>,
    leases: HashMap<String, LeaseToken>,
    id_source: SequentialIdSource,
    quota_policy: ResourceQuotaPolicy,
    event_drafts: Option<Vec<EventDraft>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceQuotaPolicy {
    pub max_leases_by_ref: BTreeMap<String, u64>,
    pub max_leases_by_kind: BTreeMap<String, u64>,
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
            quota_policy: ResourceQuotaPolicy::default(),
            event_drafts: None,
        }
    }

    pub fn with_id_source(id_source: SequentialIdSource) -> Self {
        Self {
            records: HashMap::new(),
            leases: HashMap::new(),
            id_source,
            quota_policy: ResourceQuotaPolicy::default(),
            event_drafts: None,
        }
    }

    pub(crate) fn with_runtime_event_drafts() -> Self {
        Self {
            event_drafts: Some(Vec::new()),
            ..Self::new()
        }
    }

    pub fn with_quota_policy(quota_policy: ResourceQuotaPolicy) -> Self {
        Self {
            quota_policy,
            ..Self::new()
        }
    }

    pub fn set_quota_policy(&mut self, quota_policy: ResourceQuotaPolicy) {
        self.quota_policy = quota_policy;
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
        self.record_event("resource.register", resource_ref_attributes(&ref_id), None);
        ref_id
    }

    pub fn acquire(
        &mut self,
        ref_id: &str,
        requester: impl Into<String>,
    ) -> RuntimeResult<LeaseToken> {
        let record = match self.records.get(ref_id) {
            Some(record) => record,
            None => {
                let err = RuntimeError::new(
                    "ref.not_found",
                    "runtime.resource_gate",
                    format!("runtime.resource.acquire.{ref_id}"),
                );
                self.record_event(
                    "resource.acquire.error",
                    resource_ref_attributes(ref_id),
                    Some(err.clone()),
                );
                return Err(RuntimeFailure::new(err));
            }
        };
        let requester = requester.into();
        if let Some(err) = self.quota_error(record, &requester) {
            self.record_event(
                "resource.acquire.error",
                resource_ref_kind_attributes(ref_id, &record.descriptor.kind),
                Some(err.clone()),
            );
            return Err(RuntimeFailure::new(err));
        }
        let record = self
            .records
            .get_mut(ref_id)
            .expect("record exists after quota check");
        record.lease_count += 1;
        let token = LeaseToken {
            token_id: self.id_source.next_id("lease"),
            ref_id: ref_id.to_string(),
            owner: requester,
        };
        self.leases.insert(token.token_id.clone(), token.clone());
        self.record_event(
            "resource.acquire",
            resource_token_attributes(ref_id, &token.token_id),
            None,
        );
        Ok(token)
    }

    pub fn release(&mut self, token: &LeaseToken) -> RuntimeResult<()> {
        let stored = match self.leases.get(&token.token_id) {
            Some(stored) => stored,
            None => {
                let err = RuntimeError::new(
                    "ref.not_found",
                    "runtime.resource_gate",
                    format!("runtime.resource.release.{}", token.token_id),
                );
                self.record_event(
                    "resource.release.error",
                    resource_token_attributes(&token.ref_id, &token.token_id),
                    Some(err.clone()),
                );
                return Err(RuntimeFailure::new(err));
            }
        };
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
            self.record_event("resource.release.error", BTreeMap::new(), Some(err.clone()));
            return Err(RuntimeFailure::new(err));
        }
        let removed = self
            .leases
            .remove(&token.token_id)
            .expect("lease exists after prior lookup");
        if let Some(record) = self.records.get_mut(&removed.ref_id) {
            record.lease_count = record.lease_count.saturating_sub(1);
        }
        self.record_event(
            "resource.release",
            resource_token_attributes(&removed.ref_id, &removed.token_id),
            None,
        );
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

    pub(crate) fn event_drafts(&self) -> &[EventDraft] {
        self.event_drafts.as_deref().unwrap_or(&[])
    }

    pub(crate) fn drain_event_drafts(&mut self) -> Vec<EventDraft> {
        self.event_drafts
            .as_mut()
            .map(|drafts| drafts.drain(..).collect())
            .unwrap_or_default()
    }

    fn quota_error(&self, record: &ResourceRecord, requester: &str) -> Option<RuntimeError> {
        if let Some(max) = self
            .quota_policy
            .max_leases_by_ref
            .get(&record.descriptor.ref_id)
        {
            if record.lease_count >= *max {
                return Some(self.exhausted_error(
                    "ref_id",
                    record,
                    requester,
                    *max,
                    record.lease_count,
                ));
            }
        }
        if let Some(max) = self
            .quota_policy
            .max_leases_by_kind
            .get(&record.descriptor.kind)
        {
            let current = self.kind_lease_count(&record.descriptor.kind);
            if current >= *max {
                return Some(self.exhausted_error("kind", record, requester, *max, current));
            }
        }
        None
    }

    fn kind_lease_count(&self, kind: &str) -> u64 {
        self.records
            .values()
            .filter(|record| record.descriptor.kind == kind)
            .map(|record| record.lease_count)
            .sum()
    }

    fn exhausted_error(
        &self,
        dimension: &str,
        record: &ResourceRecord,
        requester: &str,
        max: u64,
        current: u64,
    ) -> RuntimeError {
        let mut err = RuntimeError::new(
            ERR_CAPABILITY_EXHAUSTED,
            "runtime.resource_gate",
            format!("runtime.resource.acquire.{}", record.descriptor.ref_id),
        );
        err.evidence.insert(
            "dimension".into(),
            ScalarValue::String(dimension.to_string()),
        );
        err.evidence.insert(
            "ref_id".into(),
            ScalarValue::String(record.descriptor.ref_id.clone()),
        );
        err.evidence.insert(
            "kind".into(),
            ScalarValue::String(record.descriptor.kind.clone()),
        );
        err.evidence
            .insert("current".into(), ScalarValue::Int(current as i64));
        err.evidence
            .insert("max".into(), ScalarValue::Int(max as i64));
        err.evidence.insert(
            "requester".into(),
            ScalarValue::String(requester.to_string()),
        );
        err
    }

    fn record_event(
        &mut self,
        name: impl Into<String>,
        attributes: BTreeMap<String, ScalarValue>,
        error: Option<RuntimeError>,
    ) {
        if let Some(event_drafts) = &mut self.event_drafts {
            event_drafts.push(EventDraft {
                kind: RuntimeEventKind::Resource,
                name: name.into(),
                agent_id: None,
                attributes,
                error,
            });
        }
    }
}

fn resource_ref_attributes(ref_id: &str) -> BTreeMap<String, ScalarValue> {
    let mut attributes = BTreeMap::new();
    attributes.insert("ref_id".into(), ScalarValue::String(ref_id.to_string()));
    attributes
}

fn resource_ref_kind_attributes(ref_id: &str, kind: &str) -> BTreeMap<String, ScalarValue> {
    let mut attributes = resource_ref_attributes(ref_id);
    attributes.insert("kind".into(), ScalarValue::String(kind.to_string()));
    attributes
}

fn resource_token_attributes(ref_id: &str, token_id: &str) -> BTreeMap<String, ScalarValue> {
    let mut attributes = resource_ref_attributes(ref_id);
    attributes.insert("token_id".into(), ScalarValue::String(token_id.to_string()));
    attributes
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
