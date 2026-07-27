use mutsuki_bot_protocol::BotTarget;
use serde_json::Value;

use crate::api::QqScene;

pub fn qq_target_from_payload(event_type: &str, data: &Value) -> BotTarget {
    if let Some(group_id) = data
        .get("group_openid")
        .or_else(|| data.get("group_id"))
        .and_then(Value::as_str)
    {
        BotTarget::Group {
            group_id: group_id.into(),
        }
    } else if let Some(user_id) = data
        .get("author")
        .and_then(|author| {
            author
                .get("user_openid")
                .or_else(|| author.get("member_openid"))
                .or_else(|| author.get("id"))
        })
        .or_else(|| data.get("group_member_openid"))
        .or_else(|| data.get("user_openid"))
        .or_else(|| data.get("openid"))
        .or_else(|| data.get("user_id"))
        .and_then(Value::as_str)
    {
        BotTarget::User {
            user_id: user_id.into(),
        }
    } else if event_type.starts_with("GROUP") {
        BotTarget::Group {
            group_id: "unknown_group".into(),
        }
    } else if event_type.starts_with("C2C") || event_type.starts_with("FRIEND") {
        BotTarget::User {
            user_id: "unknown_user".into(),
        }
    } else {
        BotTarget::platform_specific("qqbot", event_type, "unknown")
    }
}

pub fn qq_scene_and_openid(target: &BotTarget) -> Option<(QqScene, String)> {
    match target {
        BotTarget::Group { group_id } => Some((QqScene::Group, group_id.clone())),
        BotTarget::User { user_id } => Some((QqScene::C2c, user_id.clone())),
        BotTarget::PlatformSpecific { platform, kind, id }
            if platform == "qqbot" && kind == "group" =>
        {
            Some((QqScene::Group, id.clone()))
        }
        BotTarget::PlatformSpecific { platform, kind, id }
            if platform == "qqbot" && kind == "c2c" =>
        {
            Some((QqScene::C2c, id.clone()))
        }
        _ => None,
    }
}
