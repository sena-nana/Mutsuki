use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::{
    DispatchLane, ExecutionClass, ObservabilityProfile, RunnerDescriptor, RunnerPurity,
    RunnerResult, RuntimeProfile, RuntimeProfileMode, Task, TaskStatus,
};
use mutsuki_runtime_host::{
    ExecutionDomainConfig, HostRuntime, HostRuntimeCommand, HostRuntimeConfig, HostRuntimeReply,
    NativeRunner, RuntimeBootstrapper, runner_manifest,
};
use serde::Serialize;
use serde_json::json;

const PLUGIN_ID: &str = "bench.execution-domain";
const BACKGROUND_PROTOCOL: &str = "bench.background";
const INTERACTIVE_PROTOCOL: &str = "bench.interactive";

#[derive(Clone, Debug)]
struct Options {
    samples: usize,
    background_ms: u64,
    output: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
struct Distribution {
    samples: usize,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

#[derive(Debug, Serialize)]
struct Report {
    schema: &'static str,
    business_purpose: &'static str,
    workload: serde_json::Value,
    single_path: Distribution,
    multi_path: Distribution,
    p99_improvement_percent: f64,
    expected_minimum_improvement_percent: f64,
    passed: bool,
}

fn main() -> Result<(), String> {
    let options = parse_options()?;
    if options.samples < 5 || options.background_ms == 0 {
        return Err("samples must be at least 5 and background-ms must be positive".into());
    }
    let single_path = run_case(&options, false)?;
    let multi_path = run_case(&options, true)?;
    let improvement =
        (single_path.p99_ms - multi_path.p99_ms) / single_path.p99_ms.max(f64::EPSILON) * 100.0;
    let report = Report {
        schema: "mutsuki.execution-domain-qos.v1",
        business_purpose: "complete one interactive request while the same background operation is running",
        workload: json!({
            "samples": options.samples,
            "background_block_ms": options.background_ms,
            "interactive_work": "deterministic no-op completion",
            "single_path_threads": 1,
            "multi_path_threads": {"interactive": 1, "background": 1},
            "measurement": "interactive submit to terminal outcome"
        }),
        single_path,
        multi_path,
        p99_improvement_percent: improvement,
        expected_minimum_improvement_percent: 50.0,
        passed: improvement >= 50.0,
    };
    let encoded = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    println!("{encoded}");
    if let Some(output) = options.output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(output, format!("{encoded}\n")).map_err(|error| error.to_string())?;
    }
    if !report.passed {
        return Err(format!(
            "multi-path p99 improvement {:.2}% is below the required 50%",
            report.p99_improvement_percent
        ));
    }
    Ok(())
}

fn run_case(options: &Options, multi_path: bool) -> Result<Distribution, String> {
    let mut samples = Vec::with_capacity(options.samples);
    for index in 0..options.samples {
        let runtime = runtime(multi_path, options.background_ms)?;
        let background_id = format!("background-{multi_path}-{index}");
        let mut background = Task::new(&background_id, BACKGROUND_PROTOCOL, json!({}));
        background.dispatch_lane = DispatchLane::Bulk;
        submit(&runtime, background)?;
        wait_running(&runtime, &background_id)?;

        let interactive_id = format!("interactive-{multi_path}-{index}");
        let mut interactive = Task::new(&interactive_id, INTERACTIVE_PROTOCOL, json!({}));
        interactive.dispatch_lane = DispatchLane::Interactive;
        let started = Instant::now();
        let handle = submit(&runtime, interactive)?;
        let states = runtime
            .wait_task_states(vec![handle], Duration::from_secs(2))
            .map_err(|error| error.to_string())?;
        if states.first().and_then(|state| state.status.clone()) != Some(TaskStatus::Completed) {
            return Err(format!(
                "interactive task {interactive_id} did not complete"
            ));
        }
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        let _ = runtime.wait_task_states(
            vec![task_handle(&background_id, BACKGROUND_PROTOCOL)],
            Duration::from_secs(2),
        );
    }
    Ok(distribution(samples))
}

fn runtime(multi_path: bool, background_ms: u64) -> Result<HostRuntime, String> {
    let background_descriptor = descriptor(
        "bench.background.runner",
        BACKGROUND_PROTOCOL,
        ExecutionClass::Blocking,
    );
    let interactive_descriptor = descriptor(
        "bench.interactive.runner",
        INTERACTIVE_PROTOCOL,
        ExecutionClass::Cpu,
    );
    let mut bootstrapper = RuntimeBootstrapper::new();
    bootstrapper.register_manifest(runner_manifest(
        PLUGIN_ID,
        vec![
            background_descriptor.clone(),
            interactive_descriptor.clone(),
        ],
    ));
    bootstrapper.register_runner(Box::new(NativeRunner::new(
        background_descriptor,
        move |_ctx, task| {
            thread::sleep(Duration::from_millis(background_ms));
            Ok(RunnerResult::completed(task.task_id))
        },
    )));
    bootstrapper.register_runner(Box::new(NativeRunner::new(
        interactive_descriptor,
        |_ctx, task| Ok(RunnerResult::completed(task.task_id)),
    )));
    let execution_domains = if multi_path {
        vec![
            ExecutionDomainConfig::new(
                "interactive",
                vec![ExecutionClass::Orchestration, ExecutionClass::Cpu],
                1,
            ),
            ExecutionDomainConfig::new(
                "background",
                vec![
                    ExecutionClass::Io,
                    ExecutionClass::Blocking,
                    ExecutionClass::Script,
                ],
                1,
            ),
        ]
    } else {
        vec![ExecutionDomainConfig::new(
            "shared",
            vec![
                ExecutionClass::Orchestration,
                ExecutionClass::Io,
                ExecutionClass::Cpu,
                ExecutionClass::Blocking,
                ExecutionClass::Script,
            ],
            1,
        )]
    };
    bootstrapper
        .into_host_runtime_with_config(
            profile(),
            HostRuntimeConfig {
                event_driven: true,
                execution_domains,
                ..HostRuntimeConfig::default()
            },
        )
        .map_err(|error| error.to_string())
}

fn descriptor(
    runner_id: &str,
    protocol_id: &str,
    execution_class: ExecutionClass,
) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: runner_id.into(),
        plugin_id: PLUGIN_ID.into(),
        plugin_generation: 1,
        accepted_protocol_ids: vec![protocol_id.into()],
        purity: RunnerPurity::Pure,
        execution_class,
        invocation_mode: Default::default(),
        concurrency: Default::default(),
        input_schema: json!({}),
        output_schema: json!({}),
        batch: Default::default(),
        payload: Default::default(),
        resources: Default::default(),
        ordering: Default::default(),
        control: Default::default(),
        metadata: BTreeMap::new(),
        contract_surfaces: vec![format!("runner:{runner_id}")],
    }
}

fn profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "execution-domain-qos".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![PLUGIN_ID.into()],
        bindings: BTreeMap::new(),
        plugin_deployments: BTreeMap::new(),
        observability: ObservabilityProfile::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: false,
    }
}

fn submit(
    runtime: &HostRuntime,
    task: Task,
) -> Result<mutsuki_runtime_contracts::TaskHandle, String> {
    match runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(task)))
        .map_err(|error| error.to_string())?
    {
        HostRuntimeReply::TaskSubmitted(handle) => Ok(handle),
        _ => Err("unexpected submit reply".into()),
    }
}

fn wait_running(runtime: &HostRuntime, task_id: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if runtime.task_status(task_id) == Some(TaskStatus::Running) {
            return Ok(());
        }
        thread::yield_now();
    }
    Err(format!("background task {task_id} did not start"))
}

fn task_handle(task_id: &str, protocol_id: &str) -> mutsuki_runtime_contracts::TaskHandle {
    mutsuki_runtime_contracts::TaskHandle {
        task_id: task_id.into(),
        protocol_id: protocol_id.into(),
        target_binding_id: None,
        cancel_policy: mutsuki_runtime_contracts::CancelPolicy::Cascade,
        trace_id: None,
        correlation_id: None,
    }
}

fn distribution(mut values: Vec<f64>) -> Distribution {
    values.sort_by(f64::total_cmp);
    Distribution {
        samples: values.len(),
        p50_ms: percentile(&values, 0.50),
        p95_ms: percentile(&values, 0.95),
        p99_ms: percentile(&values, 0.99),
        max_ms: *values.last().unwrap_or(&0.0),
    }
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    let index = ((values.len().saturating_sub(1)) as f64 * percentile).ceil() as usize;
    values[index.min(values.len().saturating_sub(1))]
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        samples: 30,
        background_ms: 20,
        output: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--samples" => {
                options.samples = args
                    .next()
                    .ok_or("--samples requires a value")?
                    .parse()
                    .map_err(|_| "invalid --samples")?;
            }
            "--background-ms" => {
                options.background_ms = args
                    .next()
                    .ok_or("--background-ms requires a value")?
                    .parse()
                    .map_err(|_| "invalid --background-ms")?;
            }
            "--output" => {
                options.output = Some(PathBuf::from(
                    args.next().ok_or("--output requires a path")?,
                ));
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    Ok(options)
}
