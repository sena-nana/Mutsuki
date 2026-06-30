use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{ERR_STATE_CONFLICT, ScalarValue, StateDelta, VersionExpectation};
use serde_json::Value;

use crate::RuntimeResult;

#[derive(Clone, Debug, Default)]
pub(crate) struct StateStore {
    values: BTreeMap<String, (u64, Value)>,
}

impl StateStore {
    pub(crate) fn apply(&mut self, delta: &StateDelta) -> RuntimeResult<()> {
        let current_version = self.current_version(&delta.target_ref);
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
                    ScalarValue::String(expectation.ref_id.clone()),
                );
                error.evidence.insert(
                    "expected_version".into(),
                    ScalarValue::Int(expectation.expected_version as i64),
                );
                error.evidence.insert(
                    "actual_version".into(),
                    ScalarValue::Int(current_version as i64),
                );
                return Err(crate::RuntimeFailure::new(error));
            }
        }
        Ok(())
    }

    fn current_version(&self, ref_id: &str) -> u64 {
        self.values
            .get(ref_id)
            .map(|(version, _)| *version)
            .unwrap_or(0)
    }
}
