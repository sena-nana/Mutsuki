use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    RunnerDescriptor, RunnerPurity, RuntimeEventKind, ScalarValue, Task,
};

use crate::RuntimeResult;

use super::{CoreRuntime, InvocationPollution, RunningInvocationDisposition};

impl CoreRuntime {
    pub fn running_invocations(&self) -> Vec<RunningInvocationDisposition> {
        self.classify_running_invocations()
    }

    pub fn cancel_invocation(
        &mut self,
        runner_id: &str,
        invocation_id: &str,
    ) -> RuntimeResult<usize> {
        self.registry.cancel_runner(runner_id, invocation_id)?;
        let returned = self
            .tasks
            .cancel_running_invocation(runner_id, invocation_id);
        self.events.record(
            RuntimeEventKind::Runner,
            "runner.cancel",
            Some(runner_id.to_string()),
            invocation_attrs(runner_id, invocation_id, returned),
            None,
        );
        Ok(returned)
    }

    pub(super) fn classify_running_invocations(&self) -> Vec<RunningInvocationDisposition> {
        self.tasks
            .running_records()
            .into_iter()
            .filter_map(|record| {
                let runner_id = record.claimed_by.as_ref()?;
                let descriptor = self.registry.descriptor(runner_id);
                Some(match descriptor {
                    Some(descriptor) => RunningInvocationDisposition {
                        task_id: record.task.task_id.clone(),
                        runner_id: runner_id.clone(),
                        plugin_id: descriptor.plugin_id.clone(),
                        plugin_generation: descriptor.plugin_generation,
                        pollution: classify_pollution(&record.task, &descriptor),
                    },
                    None => RunningInvocationDisposition {
                        task_id: record.task.task_id.clone(),
                        runner_id: runner_id.clone(),
                        plugin_id: "unknown".into(),
                        plugin_generation: record.task.registry_generation,
                        pollution: InvocationPollution::UnknownDirty,
                    },
                })
            })
            .collect()
    }
}

fn classify_pollution(task: &Task, runner: &RunnerDescriptor) -> InvocationPollution {
    if task.protocol_id.starts_with("effect.") || runner.purity == RunnerPurity::Effectful {
        return InvocationPollution::Polluted;
    }
    if task.protocol_id.starts_with("core.") || runner.purity == RunnerPurity::Committer {
        return InvocationPollution::Polluted;
    }
    if runner.purity != RunnerPurity::Pure {
        return InvocationPollution::UnknownDirty;
    }
    if !task.input_refs.is_empty() || !task.expected_versions.is_empty() {
        return InvocationPollution::LocalDirty;
    }
    InvocationPollution::Clean
}

fn invocation_attrs(
    runner_id: &str,
    invocation_id: &str,
    returned_to_ready: usize,
) -> BTreeMap<String, ScalarValue> {
    let mut attrs = BTreeMap::new();
    attrs.insert("runner_id".into(), ScalarValue::String(runner_id.into()));
    attrs.insert(
        "invocation_id".into(),
        ScalarValue::String(invocation_id.into()),
    );
    attrs.insert(
        "returned_to_ready".into(),
        ScalarValue::Int(returned_to_ready as i64),
    );
    attrs
}

pub(super) fn cancel_attrs(
    disposition: &RunningInvocationDisposition,
    policy: &str,
) -> BTreeMap<String, ScalarValue> {
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "runner_id".into(),
        ScalarValue::String(disposition.runner_id.clone()),
    );
    attrs.insert(
        "invocation_id".into(),
        ScalarValue::String(disposition.task_id.clone()),
    );
    attrs.insert(
        "plugin_id".into(),
        ScalarValue::String(disposition.plugin_id.clone()),
    );
    attrs.insert(
        "plugin_generation".into(),
        ScalarValue::Int(disposition.plugin_generation as i64),
    );
    attrs.insert(
        "pollution".into(),
        ScalarValue::String(format!("{:?}", disposition.pollution)),
    );
    attrs.insert("policy".into(), ScalarValue::String(policy.into()));
    attrs
}
