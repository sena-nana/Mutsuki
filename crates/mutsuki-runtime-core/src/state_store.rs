use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{ERR_STATE_CONFLICT, StateDelta};
use serde_json::Value;

use crate::RuntimeResult;

#[derive(Clone, Debug, Default)]
pub(crate) struct StateStore {
    values: BTreeMap<String, (u64, Value)>,
}

impl StateStore {
    pub(crate) fn apply(&mut self, delta: &StateDelta) -> RuntimeResult<()> {
        let current_version = self
            .values
            .get(&delta.target_ref)
            .map(|(version, _)| *version)
            .unwrap_or(0);
        if current_version != delta.expected_version {
            return Err(crate::runtime_failure(
                ERR_STATE_CONFLICT,
                "runtime.state_store",
                format!("state.commit.{}", delta.target_ref),
            ));
        }
        self.values.insert(
            delta.target_ref.clone(),
            (current_version + 1, delta.patch.clone()),
        );
        Ok(())
    }

    pub(crate) fn get(&self, ref_id: &str) -> Option<&(u64, Value)> {
        self.values.get(ref_id)
    }
}
