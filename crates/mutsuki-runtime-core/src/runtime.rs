use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ContractSurface, DomainEvent, ERR_RUNNER_PURITY_VIOLATION, ERR_STATE_CONFLICT, EffectRequest,
    RunnerDescriptor, RunnerPurity, RunnerResult, RunnerStatus, RuntimeError, RuntimeEventKind,
    RuntimeLoadPlan, ScalarValue, SpanStatus, StateDelta, SurfaceOccupancy, Task, TaskDemand,
};
use serde_json::Value;

use crate::logs::{EventLog, TraceLog};
use crate::registry::{
    DisposeBag, PluginGenerationPhase, PluginGenerationState, RegistrySnapshot, ReloadDecision,
    RunnerRegistry, compare_surfaces, validate_runtime_descriptors,
};
use crate::{ResourceManager, RuntimeFailure, RuntimeResult, TaskPool};

pub trait Runner {
    fn descriptor(&self) -> &RunnerDescriptor;

    fn step(&mut self, ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>>;

    fn cancel(&mut self, _invocation_id: &str) -> RuntimeResult<()> {
        Ok(())
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct RunnerContext {
    pub registry_generation: u64,
    pub current_step: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerLoopReport {
    pub claimed_tasks: usize,
    pub completed_tasks: usize,
}

#[derive(Clone, Debug, Default)]
struct StateStore {
    values: BTreeMap<String, (u64, Value)>,
}

impl StateStore {
    fn apply(&mut self, delta: &StateDelta) -> RuntimeResult<()> {
        let current_version = self
            .values
            .get(&delta.target_ref)
            .map(|(version, _)| *version)
            .unwrap_or(0);
        if current_version != delta.expected_version {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_STATE_CONFLICT,
                "runtime.state_store",
                format!("state.commit.{}", delta.target_ref),
            )));
        }
        self.values.insert(
            delta.target_ref.clone(),
            (current_version + 1, delta.patch.clone()),
        );
        Ok(())
    }

    fn get(&self, ref_id: &str) -> Option<&(u64, Value)> {
        self.values.get(ref_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvocationPollution {
    Clean,
    LocalDirty,
    Polluted,
    UnknownDirty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningInvocationDisposition {
    pub task_id: String,
    pub runner_id: String,
    pub plugin_id: String,
    pub plugin_generation: u64,
    pub pollution: InvocationPollution,
}

struct DrainingGeneration {
    registry_generation: u64,
    runner_ids: Vec<String>,
    registry: RunnerRegistry,
}

pub struct CoreRuntime {
    load_plan: RuntimeLoadPlan,
    task_demands: Vec<TaskDemand>,
    surfaces: Vec<ContractSurface>,
    registry: RunnerRegistry,
    draining_generations: Vec<DrainingGeneration>,
    generation_states: Vec<PluginGenerationState>,
    tasks: TaskPool,
    resources: ResourceManager,
    states: StateStore,
    events: EventLog,
    traces: TraceLog,
    current_step: u64,
}

impl CoreRuntime {
    pub fn boot(load_plan: RuntimeLoadPlan, runners: Vec<Box<dyn Runner>>) -> RuntimeResult<Self> {
        let runner_descriptors: Vec<_> = runners
            .iter()
            .map(|runner| runner.descriptor().clone())
            .collect();
        validate_runtime_descriptors(&load_plan, &runner_descriptors)?;
        let mut registry = RunnerRegistry::default();
        for runner in runners {
            registry.register(runner)?;
        }
        registry.freeze();
        let task_demands = task_demands_from_plan(&load_plan);
        let generation_states =
            generation_states_for_plan(&load_plan, PluginGenerationPhase::Active);
        Ok(Self {
            surfaces: load_plan.contract_surfaces.clone(),
            load_plan,
            task_demands,
            registry,
            draining_generations: Vec::new(),
            generation_states,
            tasks: TaskPool::default(),
            resources: ResourceManager::new(),
            states: StateStore::default(),
            events: EventLog::default(),
            traces: TraceLog::default(),
            current_step: 0,
        })
    }

    pub fn registry_snapshot(&self) -> RegistrySnapshot {
        RegistrySnapshot {
            generation: self.load_plan.registry_generation,
            frozen: true,
            runners: self.registry.descriptors(),
            task_demands: self.task_demands.clone(),
            surfaces: self.surfaces.clone(),
        }
    }

    pub fn plugin_generation_states(&self) -> &[PluginGenerationState] {
        &self.generation_states
    }

    pub fn running_invocations(&self) -> Vec<RunningInvocationDisposition> {
        self.classify_running_invocations()
    }

    pub fn draining_generation_count(&self) -> usize {
        self.draining_generations.len()
    }

    pub fn surface_occupancy(&self) -> Vec<SurfaceOccupancy> {
        merge_occupancy(
            self.tasks.surface_occupancy(),
            self.resources.surface_occupancy(&self.surfaces),
        )
    }

    pub fn enqueue_task(&mut self, mut task: Task) -> String {
        if task.registry_generation == 0 {
            task.registry_generation = self.load_plan.registry_generation;
        }
        let deprecated_surface = task
            .required_surfaces
            .iter()
            .find(|surface_id| self.is_surface_deprecated(surface_id))
            .cloned();
        let task_id = self.tasks.enqueue(task);
        if let Some(surface_id) = deprecated_surface {
            let _ = self.tasks.reject_pending(
                &task_id,
                RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_RELOAD_BLOCKED,
                    "runtime.result_router",
                    format!("surface.deprecated.{surface_id}"),
                ),
            );
        }
        self.events.record(
            RuntimeEventKind::Task,
            "task.enqueue",
            Some(task_id.clone()),
            BTreeMap::new(),
            None,
        );
        task_id
    }

    pub fn publish_raw_input(&mut self, task_id: &str, kind: &str, payload: Value) -> String {
        self.enqueue_task(Task::new(task_id, kind, payload))
    }

    pub fn tick_once(&mut self) -> RuntimeResult<RunnerLoopReport> {
        self.current_step += 1;
        let mut claimed = 0;
        let mut completed = 0;
        let descriptors = self.registry.descriptors();
        for descriptor in descriptors {
            let tasks = self.tasks.claim_ready(
                &descriptor,
                self.current_step,
                self.load_plan.registry_generation,
                8,
            );
            if tasks.is_empty() {
                continue;
            }
            claimed += tasks.len();
            if descriptor.purity == RunnerPurity::Committer && descriptor.runner_id == "core.kernel"
            {
                completed += self.process_kernel_tasks(&descriptor, tasks)?;
                continue;
            }
            let mut runner = self
                .registry
                .take_runner(&descriptor.runner_id)
                .ok_or_else(|| {
                    RuntimeFailure::new(RuntimeError::new(
                        mutsuki_runtime_contracts::ERR_RUNNER_NOT_FOUND,
                        "runtime.runner_loop",
                        format!("runner.{}", descriptor.runner_id),
                    ))
                })?;
            let ctx = RunnerContext {
                registry_generation: self.load_plan.registry_generation,
                current_step: self.current_step,
            };
            let span = self.traces.record(
                format!("trace-runner-{}", descriptor.runner_id),
                "runner.step",
                None,
                SpanStatus::Ok,
                runner_attrs(&descriptor, &self.load_plan),
            );
            self.events.record(
                RuntimeEventKind::Trace,
                "trace.span",
                Some(descriptor.runner_id.clone()),
                trace_attrs(&span),
                None,
            );
            let results = runner.step(ctx, tasks)?;
            for result in results {
                completed += self.route_result(&descriptor, result)?;
            }
            self.registry.put_runner(runner);
        }
        Ok(RunnerLoopReport {
            claimed_tasks: claimed,
            completed_tasks: completed,
        })
    }

    pub fn run_until_idle(&mut self, max_ticks: usize) -> RuntimeResult<RunnerLoopReport> {
        let mut aggregate = RunnerLoopReport {
            claimed_tasks: 0,
            completed_tasks: 0,
        };
        for _ in 0..max_ticks {
            let report = self.tick_once()?;
            aggregate.claimed_tasks += report.claimed_tasks;
            aggregate.completed_tasks += report.completed_tasks;
            if self.tasks.pending_count() == 0 && self.tasks.running_count() == 0 {
                break;
            }
        }
        Ok(aggregate)
    }

    pub fn reload(&mut self, new_plan: RuntimeLoadPlan) -> RuntimeResult<ReloadDecision> {
        let occupancy = self.surface_occupancy();
        let decision = compare_surfaces(&self.surfaces, &new_plan.contract_surfaces, &occupancy)?;
        if decision.blocked {
            return Err(RuntimeFailure::new(RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RELOAD_BLOCKED,
                "runtime.reload",
                "reload.breaking",
            )));
        }
        self.events.record(
            RuntimeEventKind::Reload,
            "plugin.reload",
            Some(new_plan.registry_generation.to_string()),
            BTreeMap::new(),
            None,
        );
        self.apply_load_plan(new_plan);
        Ok(decision)
    }

    pub fn reload_with_runners(
        &mut self,
        new_plan: RuntimeLoadPlan,
        new_runners: Vec<Box<dyn Runner>>,
    ) -> RuntimeResult<ReloadDecision> {
        let runner_descriptors: Vec<_> = new_runners
            .iter()
            .map(|runner| runner.descriptor().clone())
            .collect();
        validate_runtime_descriptors(&new_plan, &runner_descriptors)?;
        let occupancy = self.surface_occupancy();
        let decision = compare_surfaces(&self.surfaces, &new_plan.contract_surfaces, &occupancy)?;
        if decision.blocked {
            return Err(RuntimeFailure::new(RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RELOAD_BLOCKED,
                "runtime.reload",
                "reload.breaking",
            )));
        }

        let mut new_registry = RunnerRegistry::default();
        for runner in new_runners {
            new_registry.register(runner)?;
        }
        new_registry.freeze();
        for shadow_state in
            generation_states_for_plan(&new_plan, PluginGenerationPhase::ShadowStarting)
        {
            if !self.generation_states.iter().any(|state| {
                state.plugin_id == shadow_state.plugin_id
                    && state.generation == shadow_state.generation
            }) {
                self.generation_states.push(shadow_state);
            }
        }

        let old_registry_generation = self.load_plan.registry_generation;
        let dispositions = self.classify_running_invocations();
        let old_runner_ids = self.registry.runner_ids();
        for disposition in &dispositions {
            match disposition.pollution {
                InvocationPollution::Clean | InvocationPollution::LocalDirty => {
                    self.registry
                        .cancel_runner(&disposition.runner_id, &disposition.task_id)?;
                    self.tasks
                        .cancel_running_invocation(&disposition.runner_id, &disposition.task_id);
                    self.events.record(
                        RuntimeEventKind::Runner,
                        "runner.cancel",
                        Some(disposition.runner_id.clone()),
                        cancel_attrs(disposition, "reload.cancel_requeue"),
                        None,
                    );
                }
                InvocationPollution::Polluted | InvocationPollution::UnknownDirty => {
                    self.events.record(
                        RuntimeEventKind::Reload,
                        "plugin.reload.drain_invocation",
                        Some(disposition.task_id.clone()),
                        cancel_attrs(disposition, "reload.drain"),
                        None,
                    );
                }
            }
        }
        let mut old_registry = std::mem::take(&mut self.registry);
        let needs_drain = dispositions.iter().any(|disposition| {
            matches!(
                disposition.pollution,
                InvocationPollution::Polluted | InvocationPollution::UnknownDirty
            )
        });
        if needs_drain {
            self.draining_generations.push(DrainingGeneration {
                registry_generation: old_registry_generation,
                runner_ids: old_runner_ids,
                registry: old_registry,
            });
        } else {
            let _disposed = old_registry.dispose_all()?;
            self.mark_generation_phase(old_registry_generation, PluginGenerationPhase::Disposed);
        }
        self.events.record(
            RuntimeEventKind::Reload,
            "plugin.reload.swap_generation",
            Some(new_plan.registry_generation.to_string()),
            BTreeMap::new(),
            None,
        );
        self.apply_load_plan(new_plan);
        self.registry = new_registry;
        self.set_active_generation_states();
        self.settle_draining_generations()?;
        Ok(decision)
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

    pub fn dispose_plugins(&mut self) -> RuntimeResult<DisposeBag> {
        let bag = self.registry.dispose_all()?;
        self.mark_generation_phase(
            self.load_plan.registry_generation,
            PluginGenerationPhase::Disposed,
        );
        self.events.record(
            RuntimeEventKind::Plugin,
            "plugin.dispose",
            None,
            BTreeMap::new(),
            None,
        );
        Ok(bag)
    }

    pub fn settle_draining_generations(&mut self) -> RuntimeResult<DisposeBag> {
        let mut disposed = DisposeBag::default();
        let mut remaining = Vec::new();
        let mut disposed_generations = Vec::new();
        for mut generation in self.draining_generations.drain(..) {
            let still_running = generation
                .runner_ids
                .iter()
                .any(|runner_id| !self.tasks.running_records_for_runner(runner_id).is_empty());
            if still_running {
                remaining.push(generation);
            } else {
                let bag = generation.registry.dispose_all()?;
                disposed.disposed.extend(bag.disposed);
                disposed_generations.push(generation.registry_generation);
            }
        }
        self.draining_generations = remaining;
        for registry_generation in disposed_generations {
            self.mark_generation_phase(registry_generation, PluginGenerationPhase::Disposed);
        }
        Ok(disposed)
    }

    pub fn tasks(&self) -> &TaskPool {
        &self.tasks
    }

    pub fn resources(&self) -> &ResourceManager {
        &self.resources
    }

    pub fn resources_mut(&mut self) -> &mut ResourceManager {
        &mut self.resources
    }

    pub fn state_value(&self, ref_id: &str) -> Option<&(u64, Value)> {
        self.states.get(ref_id)
    }

    pub fn events(&self) -> &[mutsuki_runtime_contracts::RuntimeEvent] {
        self.events.snapshot()
    }

    pub fn trace_spans(&self) -> &[mutsuki_runtime_contracts::TraceSpan] {
        self.traces.spans()
    }

    fn is_surface_deprecated(&self, surface_id: &str) -> bool {
        self.surfaces
            .iter()
            .any(|surface| surface.surface_id == surface_id && surface.deprecated)
    }

    fn classify_running_invocations(&self) -> Vec<RunningInvocationDisposition> {
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

    fn mark_generation_phase(&mut self, registry_generation: u64, phase: PluginGenerationPhase) {
        for state in &mut self.generation_states {
            if state.generation == registry_generation {
                state.phase = phase.clone();
            }
        }
    }

    fn set_active_generation_states(&mut self) {
        let active_generation = self.load_plan.registry_generation;
        for state in &mut self.generation_states {
            if state.generation == active_generation {
                state.phase = PluginGenerationPhase::Active;
            } else if state.phase == PluginGenerationPhase::Active {
                state.phase = PluginGenerationPhase::Draining;
            }
        }
        for new_state in generation_states_for_plan(&self.load_plan, PluginGenerationPhase::Active)
        {
            if !self.generation_states.iter().any(|state| {
                state.plugin_id == new_state.plugin_id && state.generation == new_state.generation
            }) {
                self.generation_states.push(new_state);
            }
        }
    }

    fn apply_load_plan(&mut self, new_plan: RuntimeLoadPlan) {
        self.tasks.rebind_pending_generation(
            self.load_plan.registry_generation,
            new_plan.registry_generation,
        );
        self.task_demands = task_demands_from_plan(&new_plan);
        self.surfaces = new_plan.contract_surfaces.clone();
        self.load_plan = new_plan;
    }

    fn route_result(
        &mut self,
        runner: &RunnerDescriptor,
        result: RunnerResult,
    ) -> RuntimeResult<usize> {
        if runner.purity == RunnerPurity::Pure {
            for delta in result.deltas {
                self.enqueue_task(commit_task(
                    &result.task_id,
                    delta,
                    self.load_plan.registry_generation,
                ));
            }
            for effect in result.effects {
                self.enqueue_task(effect_task(
                    &result.task_id,
                    effect,
                    self.load_plan.registry_generation,
                ));
            }
        } else if runner.purity == RunnerPurity::Effectful
            && !runner.runner_id.starts_with("effect.")
        {
            return Err(RuntimeFailure::new(RuntimeError::new(
                ERR_RUNNER_PURITY_VIOLATION,
                "runtime.result_router",
                format!("runner.{}", runner.runner_id),
            )));
        }
        for event in result.events {
            self.enqueue_task(event_task(
                &result.task_id,
                event,
                self.load_plan.registry_generation,
            ));
        }
        for value_ref in result.values {
            self.events.record(
                RuntimeEventKind::Resource,
                "value.lineage",
                Some(result.task_id.clone()),
                ref_lineage_attrs(value_ref.ref_id, value_ref.schema, value_ref.generation),
                None,
            );
        }
        for resource_ref in result.resources {
            self.events.record(
                RuntimeEventKind::Resource,
                "resource.lineage",
                Some(result.task_id.clone()),
                ref_lineage_attrs(
                    resource_ref.ref_id,
                    resource_ref.schema,
                    resource_ref.generation,
                ),
                None,
            );
        }
        for task in result.tasks {
            self.enqueue_task(task);
        }
        match result.status {
            RunnerStatus::Completed => {
                self.tasks.complete(&result.task_id, &runner.runner_id)?;
                return Ok(1);
            }
            RunnerStatus::Failed => {
                self.tasks.fail(
                    &result.task_id,
                    &runner.runner_id,
                    RuntimeError::new(
                        "runner.failed",
                        "runtime.result_router",
                        format!("runner.{}", runner.runner_id),
                    ),
                )?;
                return Ok(1);
            }
            RunnerStatus::Cancelled => {
                self.tasks.cancel_task(&result.task_id, &runner.runner_id)?;
                return Ok(1);
            }
            RunnerStatus::Continue => {}
        }
        Ok(0)
    }

    fn process_kernel_tasks(
        &mut self,
        runner: &RunnerDescriptor,
        tasks: Vec<Task>,
    ) -> RuntimeResult<usize> {
        let mut completed = 0;
        for task in tasks {
            match task.kind.as_str() {
                "core.commit" => {
                    let delta: StateDelta =
                        serde_json::from_value(task.payload.clone()).map_err(|err| {
                            RuntimeFailure::new(RuntimeError::new(
                                "state.delta_decode_failed",
                                "runtime.committer",
                                err.to_string(),
                            ))
                        })?;
                    self.states.apply(&delta)?;
                    self.events.record(
                        RuntimeEventKind::State,
                        "state.commit",
                        Some(delta.target_ref),
                        BTreeMap::new(),
                        None,
                    );
                }
                "core.event.append" => {
                    let event: DomainEvent =
                        serde_json::from_value(task.payload.clone()).map_err(|err| {
                            RuntimeFailure::new(RuntimeError::new(
                                "event.decode_failed",
                                "runtime.event_log",
                                err.to_string(),
                            ))
                        })?;
                    self.events.record(
                        RuntimeEventKind::Task,
                        event.kind,
                        Some(event.event_id),
                        BTreeMap::new(),
                        None,
                    );
                }
                _ => {}
            }
            self.tasks.complete(&task.task_id, &runner.runner_id)?;
            completed += 1;
        }
        Ok(completed)
    }
}

pub struct DefaultOrchestratorRunner {
    descriptor: RunnerDescriptor,
    demands: Vec<TaskDemand>,
}

impl DefaultOrchestratorRunner {
    pub fn new(descriptor: RunnerDescriptor, demands: Vec<TaskDemand>) -> Self {
        Self {
            descriptor,
            demands,
        }
    }
}

impl Runner for DefaultOrchestratorRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        let mut results = Vec::new();
        for task in tasks {
            let mut result = RunnerResult::completed(task.task_id.clone());
            for demand in self
                .demands
                .iter()
                .filter(|demand| demand.match_rule.matches(&task))
            {
                let mut derived = Task::new(
                    format!("{}:{}", task.task_id, demand.demand_id),
                    demand.target_task_kind.clone(),
                    demand.payload_projection.clone(),
                );
                derived.priority = demand.priority;
                derived.input_refs = task.input_refs.clone();
                derived.runner_hint = demand.target_runner_hint.clone();
                derived.registry_generation = ctx.registry_generation;
                derived.correlation_id = task.correlation_id.clone();
                result.tasks.push(derived);
            }
            results.push(result);
        }
        Ok(results)
    }
}

pub struct CoreKernelRunner {
    descriptor: RunnerDescriptor,
}

impl CoreKernelRunner {
    pub fn new(plugin_generation: u64) -> Self {
        Self {
            descriptor: RunnerDescriptor {
                runner_id: "core.kernel".into(),
                plugin_id: "core".into(),
                plugin_generation,
                accepted_task_kinds: vec!["core.commit".into(), "core.event.append".into()],
                purity: RunnerPurity::Committer,
                input_schema: serde_json::json!({}),
                output_schema: serde_json::json!({}),
                metadata: BTreeMap::new(),
                contract_surfaces: vec!["runner:core.kernel".into()],
            },
        }
    }
}

impl Runner for CoreKernelRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn step(&mut self, _ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>> {
        Ok(tasks
            .into_iter()
            .map(|task| RunnerResult::completed(task.task_id))
            .collect())
    }
}

fn commit_task(source_task_id: &str, delta: StateDelta, generation: u64) -> Task {
    let mut task = Task::new(
        format!("{source_task_id}:commit"),
        "core.commit",
        serde_json::to_value(delta).expect("StateDelta serializes"),
    );
    task.registry_generation = generation;
    task
}

fn event_task(source_task_id: &str, event: DomainEvent, generation: u64) -> Task {
    let mut task = Task::new(
        format!("{source_task_id}:event:{}", event.event_id),
        "core.event.append",
        serde_json::to_value(event).expect("DomainEvent serializes"),
    );
    task.registry_generation = generation;
    task
}

fn effect_task(source_task_id: &str, effect: EffectRequest, generation: u64) -> Task {
    let mut task = Task::new(
        format!("{source_task_id}:effect:{}", effect.effect_id),
        effect.kind.clone(),
        serde_json::to_value(effect).expect("EffectRequest serializes"),
    );
    task.registry_generation = generation;
    task
}

fn task_demands_from_plan(load_plan: &RuntimeLoadPlan) -> Vec<TaskDemand> {
    load_plan
        .plugins
        .iter()
        .flat_map(|plugin| plugin.provides.task_demands.iter().cloned())
        .collect()
}

fn ref_lineage_attrs(
    ref_id: String,
    schema: String,
    generation: u64,
) -> BTreeMap<String, ScalarValue> {
    let mut attrs = BTreeMap::new();
    attrs.insert("ref_id".into(), ScalarValue::String(ref_id));
    attrs.insert("schema".into(), ScalarValue::String(schema));
    attrs.insert("generation".into(), ScalarValue::Int(generation as i64));
    attrs
}

fn runner_attrs(
    runner: &RunnerDescriptor,
    load_plan: &RuntimeLoadPlan,
) -> BTreeMap<String, ScalarValue> {
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "runner_id".into(),
        ScalarValue::String(runner.runner_id.clone()),
    );
    attrs.insert(
        "plugin_id".into(),
        ScalarValue::String(runner.plugin_id.clone()),
    );
    attrs.insert(
        "plugin_generation".into(),
        ScalarValue::Int(runner.plugin_generation as i64),
    );
    attrs.insert(
        "artifact_hash".into(),
        ScalarValue::String(
            load_plan
                .plugins
                .iter()
                .find(|plugin| plugin.plugin_id == runner.plugin_id)
                .map(|plugin| plugin.artifact.sha256.clone())
                .unwrap_or_else(|| "unknown".into()),
        ),
    );
    attrs.insert(
        "descriptor_hash".into(),
        ScalarValue::String(descriptor_fingerprint(runner)),
    );
    attrs.insert(
        "contract_fingerprint".into(),
        ScalarValue::String(contract_fingerprint(runner, load_plan)),
    );
    attrs
}

fn trace_attrs(span: &mutsuki_runtime_contracts::TraceSpan) -> BTreeMap<String, ScalarValue> {
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "trace_id".into(),
        ScalarValue::String(span.trace_id.clone()),
    );
    attrs.insert("span_id".into(), ScalarValue::String(span.span_id.clone()));
    attrs.insert("span_name".into(), ScalarValue::String(span.name.clone()));
    attrs
}

fn classify_pollution(task: &Task, runner: &RunnerDescriptor) -> InvocationPollution {
    if task.kind.starts_with("effect.") || runner.purity == RunnerPurity::Effectful {
        return InvocationPollution::Polluted;
    }
    if task.kind.starts_with("core.") || runner.purity == RunnerPurity::Committer {
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

fn generation_states_for_plan(
    load_plan: &RuntimeLoadPlan,
    phase: PluginGenerationPhase,
) -> Vec<PluginGenerationState> {
    load_plan
        .plugins
        .iter()
        .map(|plugin| PluginGenerationState {
            plugin_id: plugin.plugin_id.clone(),
            generation: load_plan.registry_generation,
            phase: phase.clone(),
        })
        .collect()
}

fn merge_occupancy(
    task_occupancy: Vec<SurfaceOccupancy>,
    resource_occupancy: Vec<SurfaceOccupancy>,
) -> Vec<SurfaceOccupancy> {
    let mut by_surface: BTreeMap<String, SurfaceOccupancy> = BTreeMap::new();
    for item in task_occupancy.into_iter().chain(resource_occupancy) {
        let entry = by_surface
            .entry(item.surface_id.clone())
            .or_insert_with(|| zero_occupancy(&item.surface_id));
        entry.pending_tasks += item.pending_tasks;
        entry.running_invocations += item.running_invocations;
        entry.resource_refs += item.resource_refs;
        entry.state_refs += item.state_refs;
        entry.active_leases += item.active_leases;
        entry.open_streams += item.open_streams;
        entry.subscriptions += item.subscriptions;
        entry.timers += item.timers;
        entry.effect_inflight += item.effect_inflight;
    }
    by_surface.into_values().collect()
}

fn zero_occupancy(surface_id: &str) -> SurfaceOccupancy {
    SurfaceOccupancy {
        surface_id: surface_id.into(),
        pending_tasks: 0,
        running_invocations: 0,
        resource_refs: 0,
        state_refs: 0,
        active_leases: 0,
        open_streams: 0,
        subscriptions: 0,
        timers: 0,
        effect_inflight: 0,
    }
}

fn invocation_attrs(
    runner_id: &str,
    invocation_id: &str,
    returned_to_pending: usize,
) -> BTreeMap<String, ScalarValue> {
    let mut attrs = BTreeMap::new();
    attrs.insert("runner_id".into(), ScalarValue::String(runner_id.into()));
    attrs.insert(
        "invocation_id".into(),
        ScalarValue::String(invocation_id.into()),
    );
    attrs.insert(
        "returned_to_pending".into(),
        ScalarValue::Int(returned_to_pending as i64),
    );
    attrs
}

fn cancel_attrs(
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

fn descriptor_fingerprint(runner: &RunnerDescriptor) -> String {
    format!(
        "runner:{}:{}:{}:{}",
        runner.runner_id,
        runner.plugin_id,
        runner.plugin_generation,
        runner.accepted_task_kinds.join(",")
    )
}

fn contract_fingerprint(runner: &RunnerDescriptor, load_plan: &RuntimeLoadPlan) -> String {
    let mut fingerprints = Vec::new();
    for surface_id in &runner.contract_surfaces {
        let fingerprint = load_plan
            .contract_surfaces
            .iter()
            .find(|surface| &surface.surface_id == surface_id)
            .map(|surface| surface.fingerprint.clone())
            .unwrap_or_else(|| "missing".into());
        fingerprints.push(format!("{surface_id}={fingerprint}"));
    }
    fingerprints.sort();
    fingerprints.join(";")
}
