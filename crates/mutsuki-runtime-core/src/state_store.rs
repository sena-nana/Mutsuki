use std::collections::{BTreeMap, VecDeque};

use mutsuki_runtime_contracts::{
    ERR_STATE_CONFLICT, ERR_STATE_HISTORY_DISABLED, ERR_STATE_HISTORY_UNAVAILABLE, RefId,
    StateDelta, StateHistoryProfile, StateRollback, VersionExpectation,
};
use serde_json::Value;

use crate::RuntimeResult;

#[derive(Clone, Debug)]
struct StateRevision {
    version: u64,
    value: Value,
    committed_at_step: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StateStore {
    values: BTreeMap<RefId, (u64, Value)>,
    history: BTreeMap<RefId, VecDeque<StateRevision>>,
    profile: StateHistoryProfile,
}

impl StateStore {
    pub(crate) fn with_profile(profile: StateHistoryProfile) -> Self {
        Self {
            profile,
            ..Self::default()
        }
    }

    pub(crate) fn configure(&mut self, profile: StateHistoryProfile, current_step: u64) {
        self.profile = profile;
        if self.profile.is_enabled() {
            self.prune(current_step);
        } else {
            self.history.clear();
        }
    }

    pub(crate) fn apply(&mut self, delta: &StateDelta, current_step: u64) -> RuntimeResult<()> {
        let current_version = self.current_version(&delta.target_ref);
        if current_version != delta.expected_version {
            return Err(crate::runtime_failure(
                ERR_STATE_CONFLICT,
                "runtime.state_store",
                format!("state.commit.{}", delta.target_ref),
            ));
        }
        let next_version = current_version + 1;
        self.values.insert(
            delta.target_ref.clone(),
            (next_version, delta.patch.clone()),
        );
        if self.profile.is_enabled() {
            self.history
                .entry(delta.target_ref.clone())
                .or_default()
                .push_back(StateRevision {
                    version: next_version,
                    value: delta.patch.clone(),
                    committed_at_step: current_step,
                });
            self.prune_ref(&delta.target_ref, current_step);
        }
        Ok(())
    }

    pub(crate) fn rollback(
        &mut self,
        request: &StateRollback,
        current_step: u64,
    ) -> RuntimeResult<u64> {
        if !self.profile.is_enabled() {
            return Err(crate::runtime_failure(
                ERR_STATE_HISTORY_DISABLED,
                "runtime.state_store",
                format!("state.rollback.disabled.{}", request.target_ref),
            ));
        }
        self.prune_ref(&request.target_ref, current_step);
        if self.current_version(&request.target_ref) == request.to_version {
            return self
                .values
                .contains_key(&request.target_ref)
                .then_some(request.to_version)
                .ok_or_else(|| {
                    crate::runtime_failure(
                        ERR_STATE_HISTORY_UNAVAILABLE,
                        "runtime.state_store",
                        format!("state.rollback.empty.{}", request.target_ref),
                    )
                });
        }
        let restored = self
            .history
            .get(&request.target_ref)
            .and_then(|history| {
                history
                    .iter()
                    .find(|revision| revision.version == request.to_version)
                    .map(|revision| revision.value.clone())
            })
            .ok_or_else(|| {
                crate::runtime_failure(
                    ERR_STATE_HISTORY_UNAVAILABLE,
                    "runtime.state_store",
                    format!("state.rollback.{}", request.target_ref),
                )
            })?;
        self.values
            .insert(request.target_ref.clone(), (request.to_version, restored));
        if let Some(history) = self.history.get_mut(&request.target_ref) {
            history.retain(|revision| revision.version <= request.to_version);
        }
        Ok(request.to_version)
    }

    pub(crate) fn prune(&mut self, current_step: u64) {
        if !self.profile.is_enabled() {
            self.history.clear();
            return;
        }
        for ref_id in self.history.keys().cloned().collect::<Vec<_>>() {
            self.prune_ref(&ref_id, current_step);
        }
    }

    pub(crate) fn get(&self, ref_id: impl AsRef<str>) -> Option<&(u64, Value)> {
        self.values.get(ref_id.as_ref())
    }

    pub(crate) fn validate_expectations(
        &self,
        expectations: &[VersionExpectation],
        route: impl Into<String>,
    ) -> RuntimeResult<()> {
        let route = route.into();
        for expectation in expectations {
            let current_version = self.current_version(&expectation.ref_id);
            if current_version != expectation.expected_version {
                let mut error =
                    crate::runtime_error(ERR_STATE_CONFLICT, "runtime.state_store", route.clone());
                error.evidence.insert(
                    "ref_id".into(),
                    mutsuki_runtime_contracts::ScalarValue::String(expectation.ref_id.to_string()),
                );
                error.evidence.insert(
                    "expected_version".into(),
                    mutsuki_runtime_contracts::ScalarValue::Int(
                        expectation.expected_version as i64,
                    ),
                );
                error.evidence.insert(
                    "actual_version".into(),
                    mutsuki_runtime_contracts::ScalarValue::Int(current_version as i64),
                );
                return Err(crate::RuntimeFailure::new(error));
            }
        }
        Ok(())
    }

    fn current_version(&self, ref_id: impl AsRef<str>) -> u64 {
        self.values
            .get(ref_id.as_ref())
            .map(|(version, _)| *version)
            .unwrap_or(0)
    }

    fn prune_ref(&mut self, ref_id: &RefId, current_step: u64) {
        let Some(history) = self.history.get_mut(ref_id) else {
            return;
        };
        while history.len() > self.profile.capacity_per_ref {
            history.pop_front();
        }
        if self.profile.retain_steps > 0 {
            while history.front().is_some_and(|revision| {
                current_step.saturating_sub(revision.committed_at_step) >= self.profile.retain_steps
            }) {
                history.pop_front();
            }
        }
        if history.is_empty() {
            self.history.remove(ref_id);
        }
    }
}
