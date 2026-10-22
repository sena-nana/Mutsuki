//! The `/bili` chat management commands and their argument parsing helpers.

use std::sync::Arc;

use mutsuki_bot_management::{
    BilibiliBindVerifyResult, BilibiliManagementApi, BilibiliNotificationKind,
};
use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotCommandEvent, BotExtMap, BotMessage, MessageSegment,
};
use mutsuki_runtime_contracts::{RunnerResult, RuntimeError, Task};

use crate::error::{bili_error, bili_management_error};
use crate::runner::decode;
use crate::{
    BilibiliConfig, BilibiliError, BilibiliHttp, BilibiliManagementService, SharedBilibiliConfig,
};

/// The `/bili` admin commands need only the management API and the shared config,
/// and `command_reply` touches no runner state at all. Cloning them out lets a
/// command that talks to upstream (subscribe resolves a profile, for instance)
/// run without the runner mutex, which used to block every poll for its duration.
pub(crate) struct BilibiliCommands {
    pub(crate) management: Option<Arc<BilibiliManagementService>>,
    pub(crate) managed_config: Option<SharedBilibiliConfig>,
}

impl BilibiliCommands {
    /// Upstream calls (login status, bind, verify) go through `http`, the runner's
    /// child-task channel.
    pub(crate) async fn run_command(
        &self,
        http: &dyn BilibiliHttp,
        task: &Task,
    ) -> Result<RunnerResult, RuntimeError> {
        let command: BotCommandEvent = decode(task)?;
        let Some(management) = self.management.clone() else {
            return Ok(RunnerResult::completed(task.task_id.clone()));
        };
        let config = self
            .managed_config
            .as_ref()
            .expect("management config is installed with the API")
            .snapshot();
        let actor_id = command
            .source
            .actor
            .as_ref()
            .map(|actor| actor.user_id.as_str())
            .ok_or_else(|| bili_error(task, BilibiliError::Forbidden))?;
        let action = command.args.first().map(String::as_str).unwrap_or("help");
        // login-status polls the QR session started by /bili login.
        if action != "login-status" && !config.management.enabled {
            return Ok(RunnerResult::completed(task.task_id.clone()));
        }
        let is_admin = config
            .management
            .admin_user_ids
            .iter()
            .any(|candidate| candidate == actor_id);
        match action {
            "help" => Ok(self.command_reply(
                task,
                &command,
                "可用命令：login、login-status、bind <uid>、verify、unbind、pause [订阅/UID]、resume [订阅/UID]、preview [订阅/UID]、list；管理员另可使用 subscribe <id> <uid> [live,dynamic,video] 与 unsubscribe <id>。",
                None,
            )),
            "login" => {
                Err(bili_error(
                    task,
                    BilibiliError::ManagementUnavailable(
                        "login requires the asynchronous image renderer".into(),
                    ),
                ))
            }
            "login-status" => {
                require_admin(is_admin).map_err(|error| bili_error(task, error))?;
                let polled = management
                    .login_poll_via(http, actor_id)
                    .await
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(task, &command, polled.message, None))
            }
            "bind" => {
                let uid = parse_uid(command.args.get(1)).map_err(|error| bili_error(task, error))?;
                let challenge = management
                    .bind_start_via(http, actor_id, uid, task.task_id.as_str())
                    .await
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    format!(
                        "已为 {} ({}) 创建验证。请临时把 {} 加入 Bilibili 个性签名，然后通过验证节点继续。",
                        challenge.name, challenge.uid, challenge.code
                    ),
                    None,
                ))
            }
            "verify" => {
                match management
                    .bind_verify_via(
                        http,
                        actor_id,
                        command.source.platform.as_str(),
                        command.source.target.clone(),
                    )
                    .await
                    .map_err(|error| bili_management_error(task, error))?
                {
                    BilibiliBindVerifyResult::Verified(subscription) => Ok(self.command_reply(
                        task,
                        &command,
                        format!(
                            "验证成功，已绑定 UID {} 并写入产品配置。",
                            subscription.uid
                        ),
                        None,
                    )),
                    BilibiliBindVerifyResult::SignatureMismatch { code } => Ok(self.command_reply(
                        task,
                        &command,
                        format!("验证未通过：个性签名中尚未找到 {code}。"),
                        None,
                    )),
                }
            }
            "unbind" => {
                let removed = management
                    .unbind(actor_id)
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    if removed {
                        "已解除绑定并更新产品配置。"
                    } else {
                        "当前没有自助绑定。"
                    },
                    None,
                ))
            }
            "pause" | "resume" => {
                let paused = action == "pause";
                let view = management
                    .set_paused(
                        actor_id,
                        is_admin,
                        command.args.get(1).map(String::as_str),
                        paused,
                    )
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    format!(
                        "订阅 {} 已{}。",
                        view.subscription_id,
                        if paused { "暂停" } else { "恢复" }
                    ),
                    None,
                ))
            }
            "preview" => {
                Err(bili_error(
                    task,
                    BilibiliError::ManagementUnavailable(
                        "preview requires the asynchronous image renderer".into(),
                    ),
                ))
            }
            "list" => {
                let lines = management
                    .list(actor_id, is_admin)
                    .map_err(|error| bili_management_error(task, error))?
                    .into_iter()
                    .map(|subscription| {
                        format!(
                            "{} -> UID {} [{}]{}",
                            subscription.subscription_id,
                            subscription.uid,
                            subscription
                                .notifications
                                .iter()
                                .map(|kind| format!("{kind:?}").to_ascii_lowercase())
                                .collect::<Vec<_>>()
                                .join(","),
                            if subscription.paused { " (paused)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>();
                Ok(self.command_reply(
                    task,
                    &command,
                    if lines.is_empty() {
                        "没有可管理的订阅。".into()
                    } else {
                        lines.join("\n")
                    },
                    None,
                ))
            }
            "subscribe" => {
                require_admin(is_admin).map_err(|error| bili_error(task, error))?;
                let subscription_id = required_arg(&command.args, 1, "subscription id")
                    .map_err(|error| bili_error(task, error))?;
                let uid = parse_uid(command.args.get(2)).map_err(|error| bili_error(task, error))?;
                let notifications = parse_notifications(command.args.get(3))
                    .map_err(|error| bili_error(task, error))?;
                management
                    .subscribe(
                        subscription_id.clone(),
                        uid,
                        notifications,
                        command.source.target.clone(),
                        config.management.self_binding_outbound_binding.clone(),
                    )
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    format!("订阅 {subscription_id} 已写入产品配置。"),
                    None,
                ))
            }
            "unsubscribe" => {
                require_admin(is_admin).map_err(|error| bili_error(task, error))?;
                let subscription_id = required_arg(&command.args, 1, "subscription id")
                    .map_err(|error| bili_error(task, error))?;
                management
                    .unsubscribe(&subscription_id)
                    .map_err(|error| bili_management_error(task, error))?;
                Ok(self.command_reply(
                    task,
                    &command,
                    format!("订阅 {subscription_id} 已从产品配置删除。"),
                    None,
                ))
            }
            _ => Ok(self.command_reply(task, &command, "未知 Bilibili 管理命令。", None)),
        }
    }

    pub(crate) fn command_reply(
        &self,
        task: &Task,
        command: &BotCommandEvent,
        text: impl Into<String>,
        image: Option<mutsuki_runtime_contracts::ResourceRef>,
    ) -> RunnerResult {
        let mut segments = Vec::new();
        if let Some(resource) = image {
            segments.push(MessageSegment::Image { resource });
        }
        segments.push(MessageSegment::Text { text: text.into() });
        let binding = self
            .managed_config
            .as_ref()
            .map(SharedBilibiliConfig::snapshot)
            .map(|config| config.management.self_binding_outbound_binding);
        command_outbound_result(
            task,
            BotMessage {
                message_id: None,
                target: command.source.target.clone(),
                sender: None,
                segments,
                reply_to: command
                    .source
                    .message
                    .as_ref()
                    .and_then(|message| message.message_id.clone()),
                time_ms: None,
                ext: BotExtMap::new(),
            },
            binding.as_deref(),
        )
    }
}

pub(crate) fn command_outbound_result(
    task: &Task,
    message: BotMessage,
    binding: Option<&str>,
) -> RunnerResult {
    let mut result = RunnerResult::completed(task.task_id.clone());
    let mut child = Task::new(
        format!("{}:reply", task.task_id),
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(message).expect("BotMessage serializes"),
    );
    child.trace_id = task.trace_id.clone();
    child.correlation_id = task
        .correlation_id
        .clone()
        .or_else(|| Some(task.task_id.to_string()));
    child.registry_generation = task.registry_generation;
    child.target_binding_id = binding.filter(|value| !value.is_empty()).map(Into::into);
    result.tasks.push(child);
    result
}

pub(crate) fn require_admin(is_admin: bool) -> Result<(), BilibiliError> {
    is_admin.then_some(()).ok_or(BilibiliError::Forbidden)
}

pub(crate) fn required_arg(
    args: &[String],
    index: usize,
    name: &str,
) -> Result<String, BilibiliError> {
    args.get(index)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| BilibiliError::InvalidResponse(format!("missing {name}")))
}

pub(crate) fn parse_uid(value: Option<&String>) -> Result<u64, BilibiliError> {
    value
        .ok_or_else(|| BilibiliError::InvalidResponse("missing Bilibili UID".into()))?
        .parse::<u64>()
        .ok()
        .filter(|uid| *uid > 0)
        .ok_or_else(|| BilibiliError::InvalidResponse("invalid Bilibili UID".into()))
}

pub(crate) fn parse_notifications(
    value: Option<&String>,
) -> Result<Vec<BilibiliNotificationKind>, BilibiliError> {
    let values = value
        .map(|value| value.split(',').collect::<Vec<_>>())
        .unwrap_or_else(|| vec!["live", "dynamic", "video"]);
    let mut notifications = Vec::new();
    for value in values {
        let kind = match value.trim() {
            "live" => BilibiliNotificationKind::Live,
            "dynamic" => BilibiliNotificationKind::Dynamic,
            "video" => BilibiliNotificationKind::Video,
            unknown => {
                return Err(BilibiliError::InvalidResponse(format!(
                    "unknown notification type {unknown}"
                )));
            }
        };
        if !notifications.contains(&kind) {
            notifications.push(kind);
        }
    }
    if notifications.is_empty() {
        return Err(BilibiliError::InvalidResponse(
            "notification types must not be empty".into(),
        ));
    }
    Ok(notifications)
}

pub(crate) fn binding_code(actor_id: &str, uid: u64, task_id: &str) -> String {
    let digest = format!("{:x}", md5::compute(format!("{actor_id}:{uid}:{task_id}")));
    format!("mutsuki-{}", &digest[..8])
}

pub(crate) fn self_subscription_id_for(platform: &str, actor_id: &str) -> String {
    let digest = format!("{:x}", md5::compute(format!("{platform}:{actor_id}")));
    format!("self-{}", &digest[..12])
}

pub(crate) fn select_subscription(
    config: &BilibiliConfig,
    actor_id: &str,
    is_admin: bool,
    selector: Option<&str>,
) -> Result<usize, BilibiliError> {
    let matches = config
        .subscriptions
        .iter()
        .enumerate()
        .filter(|(_, subscription)| {
            is_admin || subscription.owner_user_id.as_deref() == Some(actor_id)
        })
        .filter(|(_, subscription)| {
            selector.is_none_or(|selector| {
                subscription.subscription_id == selector || subscription.uid.to_string() == selector
            })
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(BilibiliError::ManagementUnavailable(
            "matching subscription was not found".into(),
        )),
        _ => Err(BilibiliError::ManagementUnavailable(
            "subscription selector is ambiguous".into(),
        )),
    }
}
