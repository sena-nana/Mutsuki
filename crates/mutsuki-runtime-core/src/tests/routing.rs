use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::*;
use serde_json::json;

use crate::*;

use super::fixtures::*;

struct ResultListRunner {
    descriptor: RunnerDescriptor,
    result: Box<dyn Fn(&Task) -> RunnerResult + Send>,
}

impl ResultListRunner {
    fn new(
        descriptor: RunnerDescriptor,
        result: impl Fn(&Task) -> RunnerResult + Send + 'static,
    ) -> Self {
        Self {
            descriptor,
            result: Box::new(result),
        }
    }
}

impl Runner for ResultListRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        scalar_batch_result(&batch, |task| Ok((self.result)(task)))
    }
}

#[test]
fn pure_runner_explicitly_enqueues_derived_tasks() {
    let orchestrator = runner_descriptor("orchestrator", "raw.input.chat", RunnerPurity::Pure);
    let plan = load_plan(vec![orchestrator.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(orchestrator, |task| {
        let mut result = RunnerResult::completed(task.task_id.clone());
        let mut derived = Task::new(
            format!("{}:memory", task.task_id),
            "sim.memory.retrieve",
            json!({"mode": "fast"}),
        );
        derived.priority = 5;
        derived.input_refs = task.input_refs.clone();
        derived.runner_hint = Some("memory.runner".into());
        derived.correlation_id = task.correlation_id.clone();
        result.tasks.push(derived);
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    runtime
        .submit_task(Task::new(
            "raw-1",
            "raw.input.chat",
            json!({"text": "hello"}),
        ))
        .unwrap();
    let report = runtime.tick_once().unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert!(runtime.tasks().get("raw-1:memory").is_some());
    assert!(
        runtime
            .trace_spans()
            .iter()
            .any(|span| span.name == "runner.run_batch")
    );
}

#[test]
fn pure_runner_outputs_are_routed_to_commit_and_effect_tasks() {
    let worker = runner_descriptor("worker", "sim.behavior.evaluate", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.deltas.push(StateDelta {
            target_ref: "state:actor".into(),
            expected_version: 0,
            patch: json!({"intent": "reply"}),
            conflict_policy: ConflictPolicy::Fail,
        });
        result.deltas.push(StateDelta {
            target_ref: "state:summary".into(),
            expected_version: 0,
            patch: json!({"status": "queued"}),
            conflict_policy: ConflictPolicy::Fail,
        });
        result.effects.push(EffectRequest {
            effect_id: "send-1".into(),
            kind: "effect.chat.send".into(),
            payload: json!({"text": "ok"}),
            preconditions: vec![EffectPrecondition {
                ref_id: "state:actor".into(),
                expected_version: 0,
            }],
            idempotency_key: Some("send-1".into()),
        });
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("task-1", "sim.behavior.evaluate", json!({})))
        .unwrap();

    runtime.tick_once().unwrap();

    assert!(runtime.tasks().get("task-1:commit:0").is_some());
    assert!(runtime.tasks().get("task-1:commit:1").is_some());
    assert!(runtime.tasks().get("task-1:effect:send-1").is_some());
}

#[test]
fn effect_preconditions_are_checked_before_effect_runner_step() {
    let worker = runner_descriptor("worker", "sim.effect.produce", RunnerPurity::Pure);
    let effect_runner =
        runner_descriptor("effect.chat", "effect.chat.send", RunnerPurity::Effectful);
    let plan = load_plan(vec![worker.clone(), effect_runner.clone()], Vec::new());
    let effect_calls = Arc::new(Mutex::new(0));
    let observed_effect_calls = effect_calls.clone();
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(
        boxed_runner!(worker, |task| {
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.effects.push(EffectRequest {
                effect_id: "send-1".into(),
                kind: "effect.chat.send".into(),
                payload: json!({}),
                preconditions: vec![EffectPrecondition {
                    ref_id: "state:actor".into(),
                    expected_version: 0,
                }],
                idempotency_key: None,
            });
            result
        }),
        boxed_runner!(effect_runner, move |task| {
            *observed_effect_calls
                .lock()
                .expect("effect calls mutex poisoned") += 1;
            RunnerResult::completed(task.task_id.clone())
        })
    );
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .submit_task(Task::new(
            "commit-1",
            "core.commit",
            serde_json::to_value(StateDelta {
                target_ref: "state:actor".into(),
                expected_version: 0,
                patch: json!({"version": 1}),
                conflict_policy: ConflictPolicy::Fail,
            })
            .unwrap(),
        ))
        .unwrap();
    runtime.tick_once().unwrap();
    runtime
        .enqueue_task(Task::new("task-1", "sim.effect.produce", json!({})))
        .unwrap();
    runtime.tick_once().unwrap();

    let report = runtime.tick_once().unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert_eq!(
        runtime.task_status("task-1:effect:send-1"),
        Some(TaskStatus::Failed)
    );
    assert_eq!(
        *effect_calls.lock().expect("effect calls mutex poisoned"),
        0
    );
}

#[test]
fn effectful_runner_cannot_return_core_derivations() {
    let effect_runner =
        runner_descriptor("effect.chat", "effect.chat.send", RunnerPurity::Effectful);
    let plan = load_plan(vec![effect_runner.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> =
        runners_with_kernel!(boxed_runner!(effect_runner, |task| {
            let mut result = RunnerResult::completed(task.task_id.clone());
            result.deltas.push(StateDelta {
                target_ref: "state:actor".into(),
                expected_version: 0,
                patch: json!({"leaked": true}),
                conflict_policy: ConflictPolicy::Fail,
            });
            result.effects.push(EffectRequest {
                effect_id: "nested".into(),
                kind: "effect.chat.send".into(),
                payload: json!({"nested": true}),
                preconditions: Vec::new(),
                idempotency_key: None,
            });
            result
        }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("effect-1", "effect.chat.send", json!({})))
        .unwrap();

    let error = runtime.tick_once().unwrap_err();

    assert_eq!(error.error().code, ERR_RUNNER_PURITY_VIOLATION);
    assert!(runtime.tasks().get("effect-1:commit:0").is_none());
    assert!(runtime.tasks().get("effect-1:effect:nested").is_none());
    assert!(runtime.state_value("state:actor").is_none());
}

#[test]
fn runner_result_value_and_resource_refs_are_recorded_as_lineage() {
    let worker = runner_descriptor("worker", "sim.resource.produce", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.values.push(ValueRef {
            ref_id: "value:1".into(),
            provider_id: "mutsuki.std.resource.memory".into(),
            schema: "value.small.v1".into(),
            version: 1,
            generation: 1,
            size_hint: Some(12),
            content_hash: Some("hash:value".into()),
            lifetime: ResourceLifetime::Persistent,
            storage: ValueStorage::LocalValueStore,
        });
        result.resources.push(ResourceRef {
            ref_id: "resource:1".into(),
            resource_id: ResourceId {
                kind_id: "bytes".into(),
                slot_id: "resource:1".into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "mutsuki.std.resource.memory".into(),
            resource_kind: "bytes".into(),
            schema: "bytes.v1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::MmapFile {
                path: "resource.bin".into(),
                offset: 0,
                len: 3,
                readonly: true,
            },
            size_hint: Some(3),
            content_hash: Some("hash:resource".into()),
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        });
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("task-refs", "sim.resource.produce", json!({})))
        .unwrap();

    runtime.tick_once().unwrap();

    assert!(runtime.events().iter().any(|event| {
        event.kind == RuntimeEventKind::Resource && event.name == "value.lineage"
    }));
    assert!(runtime.events().iter().any(|event| {
        event.kind == RuntimeEventKind::Resource && event.name == "resource.lineage"
    }));
}

#[test]
fn committer_task_is_the_only_state_store_mutation_path() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!();
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    let delta = StateDelta {
        target_ref: "state:actor".into(),
        expected_version: 0,
        patch: json!({"ok": true}),
        conflict_policy: ConflictPolicy::Fail,
    };
    runtime
        .enqueue_task(Task::new(
            "commit-1",
            "core.commit",
            serde_json::to_value(delta).unwrap(),
        ))
        .unwrap();

    runtime.tick_once().unwrap();

    assert_eq!(
        runtime.state_value("state:actor").unwrap(),
        &(1, json!({"ok": true}))
    );
}

#[test]
fn stale_task_version_expectation_fails_before_runner_step() {
    let worker = runner_descriptor("worker", "sim.versioned", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let calls = Arc::new(Mutex::new(0));
    let observed_calls = calls.clone();
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, move |task| {
        *observed_calls.lock().expect("calls mutex poisoned") += 1;
        RunnerResult::completed(task.task_id.clone())
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .submit_task(Task::new(
            "commit-1",
            "core.commit",
            serde_json::to_value(StateDelta {
                target_ref: "state:actor".into(),
                expected_version: 0,
                patch: json!({"version": 1}),
                conflict_policy: ConflictPolicy::Fail,
            })
            .unwrap(),
        ))
        .unwrap();
    runtime.tick_once().unwrap();
    let mut stale = Task::new("stale-1", "sim.versioned", json!({}));
    stale.expected_versions.push(VersionExpectation {
        ref_id: "state:actor".into(),
        expected_version: 0,
    });
    runtime.submit_task(stale).unwrap();

    let report = runtime.tick_once().unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert_eq!(runtime.task_status("stale-1"), Some(TaskStatus::Failed));
    let outcome = runtime.task_outcome("stale-1").unwrap().unwrap();
    assert!(matches!(
        outcome,
        TaskOutcome::Failed { error, .. } if error.code == ERR_STATE_CONFLICT
    ));
    assert_eq!(*calls.lock().expect("calls mutex poisoned"), 0);
}

#[test]
fn stale_child_task_version_expectation_wakes_waiting_parent() {
    let parent = runner_descriptor("parent.runner", "parent.work", RunnerPurity::Pure);
    let plan = load_plan(vec![parent.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(parent, |task| {
        if task.continuation_ref.is_some() {
            return RunnerResult::completed(task.task_id.clone());
        }
        let mut child = Task::new("child-1", "child.work", json!({}));
        child.expected_versions.push(VersionExpectation {
            ref_id: "state:actor".into(),
            expected_version: 0,
        });
        await_child_result(task, child)
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .submit_task(Task::new(
            "commit-1",
            "core.commit",
            serde_json::to_value(StateDelta {
                target_ref: "state:actor".into(),
                expected_version: 0,
                patch: json!({"version": 1}),
                conflict_policy: ConflictPolicy::Fail,
            })
            .unwrap(),
        ))
        .unwrap();
    runtime.tick_once().unwrap();
    runtime
        .enqueue_task(Task::new("parent-1", "parent.work", json!({})))
        .unwrap();
    runtime.tick_once().unwrap();

    let report = runtime.tick_once().unwrap();

    assert_eq!(report.completed_tasks, 2);
    assert_eq!(runtime.task_status("child-1"), Some(TaskStatus::Failed));
    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Completed));
}

#[test]
fn runner_trace_records_plugin_generation_and_contract_facts() {
    let worker = runner_descriptor("worker", "sim.trace", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(completed_runner!(worker));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    let mut task = Task::new("trace-task", "sim.trace", json!({}));
    task.trace_id = Some("trace:task-1".into());
    task.correlation_id = Some("correlation:task-1".into());
    runtime.enqueue_task(task).unwrap();

    runtime.tick_once().unwrap();

    let span = runtime
        .trace_spans()
        .iter()
        .find(|span| {
            span.name == "runner.run_batch"
                && span.attributes.get("runner_id") == Some(&ScalarValue::String("worker".into()))
        })
        .unwrap();
    assert_eq!(
        span.attributes.get("plugin_id"),
        Some(&ScalarValue::String("plugin-a".into()))
    );
    assert_eq!(
        span.attributes.get("plugin_generation"),
        Some(&ScalarValue::Int(1))
    );
    assert_eq!(span.trace_id, "trace:task-1");
    assert_eq!(
        span.attributes.get("task_id"),
        Some(&ScalarValue::String("trace-task".into()))
    );
    assert_eq!(
        span.attributes.get("task_lease_ids"),
        Some(&ScalarValue::String("task-lease-1-trace-task".into()))
    );
    assert_eq!(
        span.attributes.get("correlation_id"),
        Some(&ScalarValue::String("correlation:task-1".into()))
    );
    assert_eq!(
        span.attributes.get("executor_id"),
        Some(&ScalarValue::String("executor:worker".into()))
    );
    assert!(span.attributes.contains_key("artifact_hash"));
    assert!(span.attributes.contains_key("descriptor_hash"));
    assert!(span.attributes.contains_key("contract_fingerprint"));
}

#[test]
fn waiting_task_is_woken_when_child_reaches_terminal_state() {
    let parent = runner_descriptor("parent.runner", "parent.work", RunnerPurity::Pure);
    let child = runner_descriptor("child.runner", "child.work", RunnerPurity::Pure);
    let plan = load_plan(vec![parent.clone(), child.clone()], Vec::new());
    let parent_lease_ids = Arc::new(Mutex::new(Vec::new()));
    let observed_parent_lease_ids = parent_lease_ids.clone();
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(
        boxed_runner!(parent, move |task| {
            observed_parent_lease_ids
                .lock()
                .expect("parent lease ids mutex poisoned")
                .push(task.lease_id.clone());
            if task.continuation_ref.is_some() {
                return RunnerResult::completed(task.task_id.clone());
            }
            await_child_result(
                task,
                Task::new("child-1", "child.work", json!({"from": task.task_id})),
            )
        }),
        completed_runner!(child)
    );
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    runtime
        .enqueue_task(Task::new("parent-1", "parent.work", json!({})))
        .unwrap();
    runtime.tick_once().unwrap();

    let parent_record = runtime.tasks().get("parent-1").unwrap();
    assert_eq!(parent_record.status, TaskStatus::Waiting);
    assert!(parent_record.lease.is_none());
    assert_eq!(
        parent_record.task.continuation_ref.as_deref(),
        Some("continuation:parent")
    );
    runtime.tick_once().unwrap();

    assert_eq!(runtime.task_status("child-1"), Some(TaskStatus::Completed));
    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Completed));
    assert!(
        runtime
            .task_events("parent-1")
            .iter()
            .any(|event| event.name == "task.wake")
    );
    let lease_ids = parent_lease_ids
        .lock()
        .expect("parent lease ids mutex poisoned");
    assert_eq!(lease_ids.len(), 2);
    assert!(lease_ids[0].is_some());
    assert!(lease_ids[1].is_some());
    assert_ne!(lease_ids[0], lease_ids[1]);
}

#[test]
fn waiting_task_with_timer_wake_resumes_at_ready_step() {
    let parent = runner_descriptor("parent.runner", "parent.work", RunnerPurity::Pure);
    let plan = load_plan(vec![parent.clone()], Vec::new());
    let parent_calls = Arc::new(Mutex::new(0));
    let observed_parent_calls = parent_calls.clone();
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(parent, move |task| {
        let mut calls = observed_parent_calls
            .lock()
            .expect("parent calls mutex poisoned");
        *calls += 1;
        if task.continuation_ref.is_some() {
            return RunnerResult::completed(task.task_id.clone());
        }
        await_child_result_with_wake(
            task,
            Task::new("child-1", "child.work", json!({})),
            WakeCondition::Timer { ready_at_step: 3 },
        )
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    runtime
        .enqueue_task(Task::new("parent-1", "parent.work", json!({})))
        .unwrap();
    runtime.tick_once().unwrap();
    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Waiting));

    runtime.tick_once().unwrap();
    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Waiting));

    let report = runtime.tick_once().unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Completed));
    assert_eq!(runtime.task_status("child-1"), Some(TaskStatus::Ready));
    assert_eq!(
        *parent_calls.lock().expect("parent calls mutex poisoned"),
        2
    );
    assert!(runtime.task_events("parent-1").iter().any(|event| {
        event.name == "task.wake"
            && event.attributes.get("reason") == Some(&ScalarValue::String("ready_at_step".into()))
    }));
}

#[test]
fn waiting_task_is_woken_when_child_fails_or_is_cancelled() {
    run_child_terminal_wake_case(RunnerStatus::Failed, TaskStatus::Failed);
    run_child_terminal_wake_case(RunnerStatus::Cancelled, TaskStatus::Cancelled);
}

#[test]
fn waiting_task_is_woken_when_child_expires_or_dead_letters() {
    run_child_core_terminal_wake_case(TaskStatus::Expired);
    run_child_core_terminal_wake_case(TaskStatus::DeadLetter);
}

#[test]
fn cancelling_waiting_parent_cascades_to_child() {
    let parent = runner_descriptor("parent.runner", "parent.work", RunnerPurity::Pure);
    let child = runner_descriptor("child.runner", "child.work", RunnerPurity::Pure);
    let plan = load_plan(vec![parent.clone(), child.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(
        boxed_runner!(parent, |task| {
            let child_task = Task::new("child-1", "child.work", json!({}));
            await_child_result(task, child_task)
        }),
        completed_runner!(child)
    );
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("parent-1", "parent.work", json!({})))
        .unwrap();
    runtime.tick_once().unwrap();

    runtime.cancel_task_by_id("parent-1").unwrap();

    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Cancelled));
    assert_eq!(runtime.task_status("child-1"), Some(TaskStatus::Cancelled));
}

#[test]
fn cancelling_waiting_parent_rejects_reserved_cancel_policy() {
    fn runtime_waiting_on_child_with(policy: CancelPolicy) -> CoreRuntime {
        let parent = runner_descriptor("parent.runner", "parent.work", RunnerPurity::Pure);
        let child = runner_descriptor("child.runner", "child.work", RunnerPurity::Pure);
        let plan = load_plan(vec![parent.clone(), child.clone()], Vec::new());
        let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(
            boxed_runner!(parent, move |task| {
                let child_task = Task::new("child-1", "child.work", json!({}));
                await_child_result_with_policy(task, child_task, policy.clone())
            }),
            completed_runner!(child)
        );
        let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
        runtime
            .enqueue_task(Task::new("parent-1", "parent.work", json!({})))
            .unwrap();
        runtime.tick_once().unwrap();
        runtime
    }

    for policy in [CancelPolicy::Detach, CancelPolicy::Shield] {
        let mut runtime = runtime_waiting_on_child_with(policy);

        let error = runtime.cancel_task_by_id("parent-1").unwrap_err();

        assert_eq!(error.error().code, "task.cancel_policy_unsupported");
        assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Waiting));
        assert_eq!(runtime.task_status("child-1"), Some(TaskStatus::Ready));
    }
}

#[test]
fn task_cannot_suspend_while_holding_mutable_resource_lease() {
    let worker = runner_descriptor("worker", "parent.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        await_child_result(task, Task::new("child-1", "child.work", json!({})))
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    let resource = runtime
        .register_resource_descriptor(external_resource_ref(
            "resource:await-block",
            "bytes",
            "bytes.v1",
            "mutsuki.std.resource.memory",
        ))
        .unwrap();
    let _lease = runtime
        .lock_resource(&resource.ref_id, "parent-1", None)
        .unwrap();
    runtime
        .enqueue_task(Task::new("parent-1", "parent.work", json!({})))
        .unwrap();

    let error = runtime.tick_once().unwrap_err();

    assert_eq!(error.error().code, "resource.lease_cross_await");
    assert!(runtime.tasks().get("child-1").is_none());
}

#[test]
fn unknown_runner_result_fails_leased_task_before_derived_tasks_are_enqueued() {
    let worker = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |_task| {
        let mut result = RunnerResult::completed("stale-task");
        result
            .tasks
            .push(Task::new("derived-task", "sim.derived", json!({})));
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("task-1", "sim.work", json!({})))
        .unwrap();

    let report = runtime.tick_once().unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert_failed_with_code(&runtime, "task-1", ERR_TASK_CLAIM_CONFLICT);
    assert!(runtime.tasks().get("derived-task").is_none());
}

#[test]
fn duplicate_derived_task_id_is_rejected_without_overwriting_existing_task() {
    let worker = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let mut result = RunnerResult::completed(task.task_id.clone());
        result
            .tasks
            .push(Task::new("child-1", "child.work", json!({"new": true})));
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new(
            "child-1",
            "child.work",
            json!({"existing": true}),
        ))
        .unwrap();
    runtime
        .enqueue_task(Task::new("parent-1", "sim.work", json!({})))
        .unwrap();

    let error = runtime.tick_once().unwrap_err();

    assert_eq!(error.error().code, ERR_TASK_DUPLICATE);
    assert_eq!(
        runtime.tasks().get("child-1").unwrap().task.payload,
        json!({"existing": true})
    );
    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Running));
}

#[test]
fn duplicate_output_task_id_rejects_result_before_partial_routing() {
    let worker = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.values.push(ValueRef {
            ref_id: "value:partial".into(),
            provider_id: "mutsuki.std.resource.memory".into(),
            schema: "value.small.v1".into(),
            version: 1,
            generation: 1,
            size_hint: Some(12),
            content_hash: Some("hash:partial".into()),
            lifetime: ResourceLifetime::Persistent,
            storage: ValueStorage::LocalValueStore,
        });
        result
            .tasks
            .push(Task::new("child-new", "child.work", json!({"new": true})));
        result.tasks.push(Task::new(
            "child-existing",
            "child.work",
            json!({"replacement": true}),
        ));
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new(
            "child-existing",
            "child.work",
            json!({"existing": true}),
        ))
        .unwrap();
    runtime
        .enqueue_task(Task::new("parent-1", "sim.work", json!({})))
        .unwrap();

    let error = runtime.tick_once().unwrap_err();

    assert_eq!(error.error().code, ERR_TASK_DUPLICATE);
    assert!(runtime.tasks().get("child-new").is_none());
    assert_eq!(
        runtime.tasks().get("child-existing").unwrap().task.payload,
        json!({"existing": true})
    );
    assert!(
        !runtime
            .events()
            .iter()
            .any(|event| event.kind == RuntimeEventKind::Resource && event.name == "value.lineage")
    );
    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Running));
}

#[test]
fn runner_dispatch_unknown_result_fails_leased_task_without_retry() {
    let worker = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let calls = Arc::new(Mutex::new(0));
    let observed_calls = calls.clone();
    let runners: Vec<Box<dyn Runner>> =
        runners_with_kernel!(Box::new(ResultListRunner::new(worker, move |_task| {
            *observed_calls.lock().expect("calls mutex poisoned") += 1;
            RunnerResult::completed("unknown-task")
        },)) as Box<dyn Runner>);
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("task-1", "sim.work", json!({})))
        .unwrap();

    let report = runtime.tick_once().unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert_failed_with_code(&runtime, "task-1", ERR_TASK_CLAIM_CONFLICT);

    let report = runtime.tick_once().unwrap();
    assert_eq!(report.completed_tasks, 0);
    assert_eq!(*calls.lock().expect("calls mutex poisoned"), 1);
}

#[test]
fn mismatched_runner_result_fails_before_partial_routing() {
    let worker = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> =
        runners_with_kernel!(Box::new(ResultListRunner::new(worker, |_task| {
            let mut result = RunnerResult::completed("unknown-task");
            result
                .tasks
                .push(Task::new("child-duplicate-result", "child.work", json!({})));
            result.values.push(ValueRef {
                ref_id: "value:duplicate-result".into(),
                provider_id: "mutsuki.std.resource.memory".into(),
                schema: "value.small.v1".into(),
                version: 1,
                generation: 1,
                size_hint: None,
                content_hash: None,
                lifetime: ResourceLifetime::Persistent,
                storage: ValueStorage::LocalValueStore,
            });
            result
        },)) as Box<dyn Runner>);
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("task-1", "sim.work", json!({})))
        .unwrap();

    let report = runtime.tick_once().unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert_failed_with_code(&runtime, "task-1", ERR_TASK_CLAIM_CONFLICT);
    assert!(runtime.tasks().get("child-duplicate-result").is_none());
    assert!(
        !runtime
            .events()
            .iter()
            .any(|event| event.kind == RuntimeEventKind::Resource && event.name == "value.lineage")
    );
}

#[test]
fn task_await_requires_waiting_status_before_child_task_is_enqueued() {
    let worker = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let child = Task::new("child-1", "child.work", json!({}));
        let mut result = await_child_result(task, child);
        result.status = RunnerStatus::Completed;
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("task-1", "sim.work", json!({})))
        .unwrap();

    let error = runtime.tick_once().unwrap_err();

    assert_eq!(error.error().code, ERR_TASK_CLAIM_CONFLICT);
    assert!(runtime.tasks().get("child-1").is_none());
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Running));
}

#[test]
fn task_await_requires_child_task_record_before_parent_waits() {
    let worker = runner_descriptor("worker", "parent.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let mut result = runner_result_with_status(task, RunnerStatus::Waiting);
        result.task_await = Some(TaskAwait {
            parent_task_id: task.task_id.clone(),
            child: TaskHandle {
                task_id: "child-missing".into(),
                protocol_id: "child.work".into(),
                target_binding_id: None,
                cancel_policy: CancelPolicy::Cascade,
                trace_id: task.trace_id.clone(),
                correlation_id: task.correlation_id.clone(),
            },
            continuation: test_continuation("continuation:parent"),
            cancel_policy: CancelPolicy::Cascade,
        });
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("parent-1", "parent.work", json!({})))
        .unwrap();

    let error = runtime.tick_once().unwrap_err();

    assert_eq!(error.error().code, ERR_TASK_NOT_FOUND);
    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Running));
}

#[test]
fn task_await_requires_child_task_to_inherit_parent_context() {
    let worker = runner_descriptor("worker", "parent.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let child = Task::new("child-1", "child.work", json!({}));
        await_child_result(task, child)
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    let mut parent = Task::new("parent-1", "parent.work", json!({}));
    parent.trace_id = Some("trace-1".into());
    parent.correlation_id = Some("corr-1".into());
    runtime.enqueue_task(parent).unwrap();

    let error = runtime.tick_once().unwrap_err();

    assert_eq!(error.error().code, ERR_TASK_CLAIM_CONFLICT);
    assert!(runtime.tasks().get("child-1").is_none());
    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Running));
}

#[test]
fn continue_result_keeps_task_running_until_lease_expiry_reclaims_it() {
    let worker = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let calls = Arc::new(Mutex::new(0));
    let observed_calls = calls.clone();
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, move |task| {
        let mut calls = observed_calls.lock().expect("calls mutex poisoned");
        *calls += 1;
        if *calls == 1 {
            return runner_result_with_status(task, RunnerStatus::Continue);
        }
        RunnerResult::completed(task.task_id.clone())
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("task-1", "sim.work", json!({})))
        .unwrap();

    let report = runtime.tick_once().unwrap();
    assert_eq!(report.completed_tasks, 0);
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Running));

    let report = runtime.tick_once().unwrap();
    assert_eq!(report.completed_tasks, 1);
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Completed));
    assert_eq!(*calls.lock().expect("calls mutex poisoned"), 2);
}

#[test]
fn continue_result_rejects_outputs_before_partial_routing() {
    let worker = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(worker, |task| {
        let mut result = runner_result_with_status(task, RunnerStatus::Continue);
        result
            .tasks
            .push(Task::new("child-continue", "child.work", json!({})));
        result.values.push(ValueRef {
            ref_id: "value:continue".into(),
            provider_id: "mutsuki.std.resource.memory".into(),
            schema: "value.small.v1".into(),
            version: 1,
            generation: 1,
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            storage: ValueStorage::LocalValueStore,
        });
        result
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("task-1", "sim.work", json!({})))
        .unwrap();

    let error = runtime.tick_once().unwrap_err();

    assert_eq!(error.error().code, ERR_TASK_CLAIM_CONFLICT);
    assert!(runtime.tasks().get("child-continue").is_none());
    assert!(
        !runtime
            .events()
            .iter()
            .any(|event| event.kind == RuntimeEventKind::Resource && event.name == "value.lineage")
    );
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Running));
}

#[test]
fn failed_runner_step_keeps_runner_registered_for_retry() {
    struct FailsFirstRunner {
        descriptor: RunnerDescriptor,
        calls: Arc<Mutex<usize>>,
    }

    impl Runner for FailsFirstRunner {
        fn descriptor(&self) -> &RunnerDescriptor {
            &self.descriptor
        }

        fn run_batch(
            &mut self,
            _ctx: RunnerContext,
            batch: WorkBatch,
        ) -> RuntimeResult<CompletionBatch> {
            let mut calls = self.calls.lock().expect("calls mutex poisoned");
            *calls += 1;
            if *calls == 1 {
                return Err(crate::runtime_failure(
                    "runner.step_failed",
                    "test.runner",
                    "first call fails",
                ));
            }
            scalar_batch_result(&batch, |task| {
                Ok(RunnerResult::completed(task.task_id.clone()))
            })
        }
    }

    let worker = runner_descriptor("worker", "sim.work", RunnerPurity::Pure);
    let plan = load_plan(vec![worker.clone()], Vec::new());
    let calls = Arc::new(Mutex::new(0));
    let runners: Vec<Box<dyn Runner>> = vec![
        Box::new(FailsFirstRunner {
            descriptor: worker,
            calls: calls.clone(),
        }),
        kernel_runner!(1),
    ];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("task-1", "sim.work", json!({})))
        .unwrap();

    let report = runtime.tick_once().unwrap();
    assert_eq!(report.completed_tasks, 1);
    assert_eq!(runtime.task_status("task-1"), Some(TaskStatus::Failed));
    assert_eq!(
        runtime
            .task_result("task-1")
            .unwrap()
            .failure
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("runner.step_failed")
    );
    assert!(
        runtime
            .registry_snapshot()
            .runners
            .iter()
            .any(|runner| runner.runner_id == "worker")
    );
    assert_eq!(*calls.lock().expect("calls mutex poisoned"), 1);
}

#[test]
fn task_handle_facade_exposes_status_result_cancel_and_events() {
    let plan = load_plan(Vec::new(), Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!();
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    let handle = runtime
        .submit_task(Task::new("handle-task", "manual.work", json!({})))
        .unwrap();

    assert_eq!(runtime.task_handle_status(&handle), Some(TaskStatus::Ready));
    assert_eq!(
        runtime.task_handle_result(&handle).unwrap().status,
        TaskStatus::Ready
    );
    assert!(runtime.task_handle_outcome(&handle).unwrap().is_none());

    runtime.cancel_task_handle(&handle).unwrap();

    assert_eq!(
        runtime.task_handle_outcome(&handle).unwrap(),
        Some(TaskOutcome::Cancelled {
            task_id: "handle-task".into(),
            reason: None,
        })
    );
    assert!(
        runtime
            .task_handle_events(&handle)
            .iter()
            .any(|event| event.name == "task.cancelled")
    );
}

#[test]
fn core_kernel_fails_unknown_core_task() {
    let kernel = runner_descriptor("core.kernel", "core.unknown", RunnerPurity::Committer);
    let plan = load_plan(vec![kernel.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = vec![completed_runner!(kernel)];
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();
    runtime
        .enqueue_task(Task::new("core-unknown", "core.unknown", json!({})))
        .unwrap();

    let report = runtime.tick_once().unwrap();

    assert_eq!(report.completed_tasks, 1);
    assert_eq!(
        runtime.task_status("core-unknown"),
        Some(TaskStatus::Failed)
    );
    let outcome = runtime.task_outcome("core-unknown").unwrap().unwrap();
    assert!(matches!(outcome, TaskOutcome::Failed { .. }));
}

fn run_child_terminal_wake_case(
    child_runner_status: RunnerStatus,
    expected_child_status: TaskStatus,
) {
    let parent = runner_descriptor("parent.runner", "parent.work", RunnerPurity::Pure);
    let child = runner_descriptor("child.runner", "child.work", RunnerPurity::Pure);
    let plan = load_plan(vec![parent.clone(), child.clone()], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(
        boxed_runner!(parent, |task| {
            if task.continuation_ref.is_some() {
                return RunnerResult::completed(task.task_id.clone());
            }
            let child_task = Task::new("child-1", "child.work", json!({}));
            await_child_result(task, child_task)
        }),
        boxed_runner!(child, move |task| {
            runner_result_with_status(task, child_runner_status.clone())
        })
    );
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    runtime
        .enqueue_task(Task::new("parent-1", "parent.work", json!({})))
        .unwrap();
    runtime.tick_once().unwrap();
    runtime.tick_once().unwrap();

    assert_eq!(runtime.task_status("child-1"), Some(expected_child_status));
    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Completed));
    assert!(
        runtime
            .task_events("parent-1")
            .iter()
            .any(|event| event.name == "task.wake")
    );
}

fn run_child_core_terminal_wake_case(child_status: TaskStatus) {
    let parent = runner_descriptor("parent.runner", "parent.work", RunnerPurity::Pure);
    let child = runner_descriptor("child.runner", "child.work", RunnerPurity::Pure);
    let plan = load_plan(vec![parent.clone(), child], Vec::new());
    let runners: Vec<Box<dyn Runner>> = runners_with_kernel!(boxed_runner!(parent, |task| {
        if task.continuation_ref.is_some() {
            return RunnerResult::completed(task.task_id.clone());
        }
        let child_task = Task::new("child-1", "child.work", json!({}));
        await_child_result(task, child_task)
    }));
    let mut runtime = CoreRuntime::boot(plan, runners).unwrap();

    runtime
        .enqueue_task(Task::new("parent-1", "parent.work", json!({})))
        .unwrap();
    runtime.tick_once().unwrap();
    match child_status {
        TaskStatus::Expired => runtime.expire_task("child-1", "deadline").unwrap(),
        TaskStatus::DeadLetter => runtime.dead_letter_task("child-1", "retries").unwrap(),
        _ => unreachable!("test only covers core terminal states"),
    }
    runtime.tick_once().unwrap();

    assert_eq!(runtime.task_status("child-1"), Some(child_status.clone()));
    assert_eq!(runtime.task_status("parent-1"), Some(TaskStatus::Completed));
    match runtime.task_outcome("child-1").unwrap().unwrap() {
        TaskOutcome::Expired { .. } => assert_eq!(child_status, TaskStatus::Expired),
        TaskOutcome::DeadLetter { .. } => assert_eq!(child_status, TaskStatus::DeadLetter),
        other => panic!("unexpected child outcome: {other:?}"),
    }
}

fn await_child_result(parent: &Task, child: Task) -> RunnerResult {
    await_child_result_with_policy(parent, child, CancelPolicy::Cascade)
}

fn await_child_result_with_wake(parent: &Task, child: Task, wake: WakeCondition) -> RunnerResult {
    let mut result = await_child_result_with_policy(parent, child, CancelPolicy::Cascade);
    if let Some(task_await) = &mut result.task_await {
        task_await.continuation.wake = Some(wake);
    }
    result
}

fn await_child_result_with_policy(
    parent: &Task,
    child: Task,
    cancel_policy: CancelPolicy,
) -> RunnerResult {
    let child_handle = TaskHandle {
        task_id: child.task_id.clone(),
        protocol_id: child.protocol_id.clone(),
        target_binding_id: None,
        cancel_policy: cancel_policy.clone(),
        trace_id: parent.trace_id.clone(),
        correlation_id: parent.correlation_id.clone(),
    };
    let mut result = runner_result_with_status(parent, RunnerStatus::Waiting);
    result.tasks.push(child);
    result.task_await = Some(TaskAwait {
        parent_task_id: parent.task_id.clone(),
        child: child_handle,
        continuation: test_continuation("continuation:parent"),
        cancel_policy,
    });
    result
}

fn runner_result_with_status(task: &Task, status: RunnerStatus) -> RunnerResult {
    RunnerResult {
        task_id: task.task_id.clone(),
        deltas: Vec::new(),
        events: Vec::new(),
        tasks: Vec::new(),
        effects: Vec::new(),
        values: Vec::new(),
        resources: Vec::new(),
        task_await: None,
        status,
    }
}

fn assert_failed_with_code(runtime: &CoreRuntime, task_id: &str, code: &str) {
    assert_eq!(runtime.task_status(task_id), Some(TaskStatus::Failed));
    let Some(TaskOutcome::Failed { error, .. }) = runtime.task_outcome(task_id).unwrap() else {
        panic!("{task_id} should have failed");
    };
    assert_eq!(error.code, code);
}

fn test_continuation(ref_id: &str) -> TaskStepContinuation {
    TaskStepContinuation {
        continuation: ResourceRef {
            ref_id: ref_id.into(),
            resource_id: ResourceId {
                kind_id: "continuation".into(),
                slot_id: ref_id.into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "test".into(),
            resource_kind: "continuation".into(),
            schema: "continuation.v1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        },
        wake: None,
        reason: Some("await child".into()),
    }
}
