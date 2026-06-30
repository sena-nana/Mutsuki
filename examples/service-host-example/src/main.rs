use std::sync::{Arc, Mutex};

use mutsuki_runtime_contracts::{TaskOutcome, TaskStatus};
use mutsuki_runtime_host::{HostRuntimeCommand, HostRuntimeReply};
use qqbot_echo::{EchoSmokeConfig, build_bootstrapper, qqbot_group_message_task, request_log_json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = EchoSmokeConfig::from_env();
    let recording = Arc::new(Mutex::new(Vec::new()));
    let task = qqbot_group_message_task(&config);
    let (bootstrapper, profile) = build_bootstrapper(config, recording.clone());
    let mut host = bootstrapper.into_host_runtime(profile)?;
    let task_id = match host.dispatch(HostRuntimeCommand::SubmitTask(Box::new(task)))? {
        HostRuntimeReply::TaskSubmitted(task_id) => task_id,
        unexpected => return Err(format!("unexpected submit reply: {unexpected:?}").into()),
    };
    match host.dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 16 })? {
        HostRuntimeReply::Idle(report) => {
            println!(
                "runtime idle: claimed_tasks={}, completed_tasks={}",
                report.claimed_tasks, report.completed_tasks
            );
        }
        unexpected => return Err(format!("unexpected idle reply: {unexpected:?}").into()),
    }
    let status = host.task_status(&task_id);
    if status != Some(TaskStatus::Completed) {
        return Err(format!("gateway task did not complete: {status:?}").into());
    }
    let outcome = match host.dispatch(HostRuntimeCommand::TaskOutcome(task_id.clone()))? {
        HostRuntimeReply::TaskOutcome(outcome) => outcome,
        unexpected => return Err(format!("unexpected outcome reply: {unexpected:?}").into()),
    };
    if !matches!(outcome, Some(TaskOutcome::Completed { .. })) {
        return Err(format!("gateway task outcome is not completed: {outcome:?}").into());
    }
    let requests = recording.lock().unwrap();
    println!(
        "{}",
        serde_json::to_string_pretty(&request_log_json(&requests))?
    );
    Ok(())
}
