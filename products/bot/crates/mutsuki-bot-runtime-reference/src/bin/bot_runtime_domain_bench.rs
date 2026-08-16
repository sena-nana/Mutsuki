use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use mutsuki_agent_contracts::AgentMessage;
use mutsuki_bot_runtime_reference::{
    BotReferenceWorkload, BotRuntimeDomainReference, BotRuntimeTopology,
};
use mutsuki_runtime_contracts::{DomainTaskHandle, TaskOutcome, TaskStatus};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Debug)]
struct Options {
    samples: usize,
    min_background_ms: u64,
    output: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct CalibratedWork {
    session_append_iterations: usize,
    context_build_iterations: usize,
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
    workload: Value,
    single_domain: Distribution,
    bot_agent_domains: Distribution,
    p99_improvement_percent: f64,
    expected_minimum_improvement_percent: f64,
    passed: bool,
}

fn main() -> Result<(), String> {
    let options = parse_options()?;
    if options.samples < 300 || options.min_background_ms == 0 {
        return Err("samples must be at least 300 and min-background-ms must be positive".into());
    }
    let calibrated = calibrate(&options)?;
    let (single_domain, bot_agent_domains) = run_comparison(&options, calibrated)?;
    let improvement = (single_domain.p99_ms - bot_agent_domains.p99_ms)
        / single_domain.p99_ms.max(f64::EPSILON)
        * 100.0;
    let report = Report {
        schema: "mutsuki.bot.runtime-domain-reference.v1",
        business_purpose: "query QQ gateway status while Agent session and context contract work are saturated",
        workload: json!({
            "samples": options.samples,
            "minimum_background_work_ms": options.min_background_ms,
            "calibration_safety_factor": 2,
            "calibration_attempts_per_step": 3,
            "calibrated": calibrated,
            "bot_work": "deserialize the owner QqBotGatewayStatusRequest contract and return status",
            "agent_work": [
                "round-trip the owner AgentSessionAppendRequest contract",
                "round-trip the owner AgentContextBuildRequest contract"
            ],
            "single_domain_threads": 2,
            "bot_agent_domain_threads": {
                "bot": 1,
                "agent": 1
            },
            "same_total_worker_budget": 2,
            "same_protocols_runners_payloads_and_outputs": true,
            "measurement": "gateway status submit to terminal outcome",
            "runtime_lifecycle": "reuse one warmed long-lived runtime per topology",
            "sample_order": "alternate single-domain-first and bot-agent-first paired samples",
            "percentile_method": "nearest-rank"
        }),
        single_domain,
        bot_agent_domains,
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
            "bot/agent-domain p99 improvement {:.2}% is below the required 50%",
            report.p99_improvement_percent
        ));
    }
    Ok(())
}

fn calibrate(options: &Options) -> Result<CalibratedWork, String> {
    let calibration_target_ms = options
        .min_background_ms
        .checked_mul(2)
        .ok_or_else(|| "min-background-ms is too large".to_string())?;
    Ok(CalibratedWork {
        session_append_iterations: calibrate_work(
            calibration_target_ms,
            BotReferenceWorkload::AgentSessionAppend,
        )?,
        context_build_iterations: calibrate_work(
            calibration_target_ms,
            BotReferenceWorkload::AgentContextBuild,
        )?,
    })
}

fn calibrate_work(minimum_ms: u64, workload: BotReferenceWorkload) -> Result<usize, String> {
    let reference = BotRuntimeDomainReference::start(BotRuntimeTopology::SingleDomain)?;
    let warmup = reference.submit(
        format!("calibrate-warmup-{workload:?}"),
        workload,
        payload(workload, 1),
    )?;
    ensure_completed(&reference, &warmup, Duration::from_secs(30))?;
    let mut iterations = 1usize;
    loop {
        let mut fastest = Duration::MAX;
        for attempt in 0..3 {
            let started = Instant::now();
            let handle = reference.submit(
                format!("calibrate-{workload:?}-{iterations}-{attempt}"),
                workload,
                payload(workload, iterations),
            )?;
            ensure_completed(&reference, &handle, Duration::from_secs(30))?;
            fastest = fastest.min(started.elapsed());
        }
        if fastest >= Duration::from_millis(minimum_ms) {
            return Ok(iterations);
        }
        iterations = iterations
            .checked_mul(2)
            .filter(|value| *value <= 1_048_576)
            .ok_or_else(|| format!("unable to calibrate {workload:?} to {minimum_ms}ms"))?;
    }
}

fn run_comparison(
    options: &Options,
    calibrated: CalibratedWork,
) -> Result<(Distribution, Distribution), String> {
    let single = BotRuntimeDomainReference::start(BotRuntimeTopology::SingleDomain)?;
    let dual = BotRuntimeDomainReference::start(BotRuntimeTopology::BotAgentDomains)?;
    warm_up(&single, "single")?;
    warm_up(&dual, "dual")?;

    let mut single_values = Vec::with_capacity(options.samples);
    let mut dual_values = Vec::with_capacity(options.samples);
    for sample in 0..options.samples {
        if sample % 2 == 0 {
            single_values.push(run_sample(&single, "single", sample, calibrated)?);
            dual_values.push(run_sample(&dual, "dual", sample, calibrated)?);
        } else {
            dual_values.push(run_sample(&dual, "dual", sample, calibrated)?);
            single_values.push(run_sample(&single, "single", sample, calibrated)?);
        }
    }
    Ok((distribution(single_values), distribution(dual_values)))
}

fn warm_up(reference: &BotRuntimeDomainReference, topology: &str) -> Result<(), String> {
    for workload in [
        BotReferenceWorkload::GatewayStatus,
        BotReferenceWorkload::AgentSessionAppend,
        BotReferenceWorkload::AgentContextBuild,
    ] {
        let handle = reference.submit(
            format!("warmup-{topology}-{workload:?}"),
            workload,
            payload(workload, 1),
        )?;
        ensure_completed(reference, &handle, Duration::from_secs(30))?;
    }
    Ok(())
}

fn run_sample(
    reference: &BotRuntimeDomainReference,
    topology: &str,
    sample: usize,
    calibrated: CalibratedWork,
) -> Result<f64, String> {
    let append = reference.submit(
        format!("{topology}-{sample}-append"),
        BotReferenceWorkload::AgentSessionAppend,
        payload(
            BotReferenceWorkload::AgentSessionAppend,
            calibrated.session_append_iterations,
        ),
    )?;
    let context = reference.submit(
        format!("{topology}-{sample}-context"),
        BotReferenceWorkload::AgentContextBuild,
        payload(
            BotReferenceWorkload::AgentContextBuild,
            calibrated.context_build_iterations,
        ),
    )?;
    wait_running(reference, &append)?;
    if reference.is_single_domain() {
        wait_running(reference, &context)?;
    }

    let started = Instant::now();
    let gateway = reference.submit(
        format!("{topology}-{sample}-gateway"),
        BotReferenceWorkload::GatewayStatus,
        payload(BotReferenceWorkload::GatewayStatus, 1),
    )?;
    ensure_completed(reference, &gateway, Duration::from_secs(30))?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;

    for handle in [&append, &context] {
        ensure_completed(reference, handle, Duration::from_secs(30))?;
    }
    Ok(elapsed_ms)
}

fn ensure_completed(
    reference: &BotRuntimeDomainReference,
    handle: &DomainTaskHandle,
    timeout: Duration,
) -> Result<(), String> {
    match reference.wait_outcome(handle, timeout)? {
        Some(TaskOutcome::Completed {
            output: Some(_), ..
        }) => Ok(()),
        other => Err(format!(
            "task {} did not complete with business output: {other:?}",
            handle.task.task_id
        )),
    }
}

fn wait_running(
    reference: &BotRuntimeDomainReference,
    handle: &DomainTaskHandle,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let status = reference
            .group()
            .domain(&handle.domain_id)
            .and_then(|runtime| runtime.task_status(handle.task.task_id.as_str()));
        if status == Some(TaskStatus::Running) {
            return Ok(());
        }
        if matches!(
            status,
            Some(
                TaskStatus::Completed
                    | TaskStatus::Failed
                    | TaskStatus::Cancelled
                    | TaskStatus::Expired
                    | TaskStatus::DeadLetter
            )
        ) {
            return Err(format!(
                "background task {} completed before pressure was established",
                handle.task.task_id
            ));
        }
        thread::yield_now();
    }
    Err(format!(
        "background task {} did not start",
        handle.task.task_id
    ))
}

fn payload(workload: BotReferenceWorkload, iterations: usize) -> Value {
    match workload {
        BotReferenceWorkload::GatewayStatus => json!({}),
        BotReferenceWorkload::AgentSessionAppend => json!({
            "iterations": iterations,
            "request": {
                "session_id": "issue43-session",
                "messages": [AgentMessage::user(
                    "Normalize this QQ event, update the Agent session, and produce a delivery request."
                )]
            }
        }),
        BotReferenceWorkload::AgentContextBuild => json!({
            "iterations": iterations,
            "request": {
                "profile_id": "issue43-agent",
                "messages": [AgentMessage::user(
                    "Build context while the Bot gateway remains responsive."
                )],
                "session_id": "issue43-session",
                "max_context_tokens": 4096
            }
        }),
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
    let index = ((values.len() as f64 * percentile).ceil() as usize).saturating_sub(1);
    values[index.min(values.len().saturating_sub(1))]
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        samples: 300,
        min_background_ms: 20,
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
            "--min-background-ms" => {
                options.min_background_ms = args
                    .next()
                    .ok_or("--min-background-ms requires a value")?
                    .parse()
                    .map_err(|_| "invalid --min-background-ms")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_p99_excludes_the_top_one_percent() {
        let values = (1..=100).map(f64::from).collect::<Vec<_>>();
        assert_eq!(percentile(&values, 0.99), 99.0);
    }
}
