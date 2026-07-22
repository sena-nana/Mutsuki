use mutsuki_bot_protocol::{BotEvent, BotEventSubscription};
use mutsuki_runtime_contracts::Task;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotDispatchTask {
    pub task_id: String,
    pub protocol_id: String,
    pub target_binding_id: Option<String>,
}

pub fn build_dispatch_task(
    parent: &Task,
    event: &BotEvent,
    subscription: &BotEventSubscription,
    sequence: u64,
    registry_generation: u64,
) -> Task {
    let task_id = format!("mutsuki.bot.event.dispatch:{}:{}", event.event_id, sequence);
    let mut task = parent.derive_with_protocol(task_id, subscription.handler_protocol_id.clone());
    task.target_binding_id = subscription.handler_binding_id.clone();
    task.registry_generation = registry_generation;
    task
}
