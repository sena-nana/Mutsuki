use mutsuki_bot_protocol::BotEvent;
use mutsuki_runtime_contracts::Task;

use crate::BotEventSubscription;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotDispatchTask {
    pub task_id: String,
    pub protocol_id: String,
    pub target_binding_id: Option<String>,
}

pub fn build_dispatch_task(
    event: &BotEvent,
    subscription: &BotEventSubscription,
    sequence: u64,
    registry_generation: u64,
) -> Result<Task, serde_json::Error> {
    let task_id = format!("mutsuki.bot.event.dispatch:{}:{}", event.event_id, sequence);
    let mut task = Task::new(
        task_id,
        subscription.handler_protocol_id.clone(),
        serde_json::to_value(event)?,
    );
    task.target_binding_id = subscription.handler_binding_id.clone();
    task.registry_generation = registry_generation;
    Ok(task)
}
