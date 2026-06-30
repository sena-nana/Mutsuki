use mutsuki_bot_protocol::{
    BOT_MEDIA_UPLOAD_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID, BotMessage, BotTarget,
};
use mutsuki_runtime_contracts::Task;
use serde::Serialize;
use thiserror::Error;

use crate::MessageBuilder;

#[derive(Debug, Error)]
pub enum BotSdkError {
    #[error("payload serialization failed: {0}")]
    Serialize(serde_json::Error),
    #[error("task submit failed: {0}")]
    Submit(String),
}

pub trait BotTaskClient {
    fn submit_bot_task(&mut self, task: Task) -> Result<(), BotSdkError>;
}

pub struct BotContext<'a, C> {
    client: &'a mut C,
    next_task_id: u64,
}

impl<'a, C> BotContext<'a, C>
where
    C: BotTaskClient,
{
    pub fn new(client: &'a mut C) -> Self {
        Self {
            client,
            next_task_id: 0,
        }
    }

    pub fn send_message(&mut self, message: BotMessage) -> Result<String, BotSdkError> {
        self.submit(BOT_MESSAGE_SEND_PROTOCOL_ID, message)
    }

    pub fn send_text(
        &mut self,
        target: BotTarget,
        text: impl Into<String>,
    ) -> Result<String, BotSdkError> {
        self.send_message(MessageBuilder::new(target).text(text).build())
    }

    pub fn upload_media<T>(&mut self, payload: T) -> Result<String, BotSdkError>
    where
        T: Serialize,
    {
        self.submit(BOT_MEDIA_UPLOAD_PROTOCOL_ID, payload)
    }

    fn submit<T>(&mut self, protocol_id: &str, payload: T) -> Result<String, BotSdkError>
    where
        T: Serialize,
    {
        self.next_task_id += 1;
        let task_id = format!("{protocol_id}:sdk:{}", self.next_task_id);
        let payload = serde_json::to_value(payload).map_err(BotSdkError::Serialize)?;
        let task = Task::new(task_id.clone(), protocol_id, payload);
        self.client.submit_bot_task(task)?;
        Ok(task_id)
    }
}
