use std::sync::{Arc, Mutex};

use mutsuki_bot_protocol::BotTarget;
use mutsuki_bot_sdk::BotContext;
use mutsuki_runtime_contracts::{TaskOutcome, TaskStatus};
use mutsuki_runtime_host::{HostRuntimeCommand, HostRuntimeReply};
use mutsuki_runtime_sdk::HostRuntime as _;
use qqbot_echo::{EchoSmokeConfig, build_bootstrapper, qqbot_group_message_task, request_log_json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = EchoSmokeConfig::default();
    let recording = Arc::new(Mutex::new(Vec::new()));
    let task = qqbot_group_message_task(&config);
    let group_id = config.group_openid.clone();
    let (bootstrapper, profile) = build_bootstrapper(config, recording.clone());
    let host = bootstrapper.into_host_runtime(profile)?;
    let task_handle = host.submit_task(task)?;
    match host.dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 16 })? {
        HostRuntimeReply::Idle(report) => {
            println!(
                "runtime idle: claimed_tasks={}, completed_tasks={}",
                report.claimed_tasks, report.completed_tasks
            );
        }
        unexpected => return Err(format!("unexpected idle reply: {unexpected:?}").into()),
    }
    let status = host.task_status(&task_handle.task_id);
    if status != Some(TaskStatus::Completed) {
        return Err(format!("gateway task did not complete: {status:?}").into());
    }
    let outcome = host.task_outcome(&task_handle)?;
    if !matches!(outcome, Some(TaskOutcome::Completed { .. })) {
        return Err(format!("gateway task outcome is not completed: {outcome:?}").into());
    }

    let bot =
        BotContext::from_submitter(host.host_context().task_submitter_ref(), "service-host.bot");
    let direct_message = bot.send_text(BotTarget::Group { group_id }, "service host ready")?;
    let direct_report = match host.dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 16 })? {
        HostRuntimeReply::Idle(report) => report,
        unexpected => return Err(format!("unexpected idle reply: {unexpected:?}").into()),
    };
    let direct_outcome = bot.task_outcome(&direct_message)?;
    if !matches!(direct_outcome, Some(TaskOutcome::Completed { .. })) {
        return Err(format!(
            "direct Bot SDK task did not complete: status={:?}, outcome={direct_outcome:?}, claimed={}, completed={}",
            host.task_status(&direct_message.task_id),
            direct_report.claimed_tasks,
            direct_report.completed_tasks,
        )
        .into());
    }
    let requests = recording.lock().unwrap();
    println!(
        "{}",
        serde_json::to_string_pretty(&request_log_json(&requests))?
    );
    Ok(())
}
