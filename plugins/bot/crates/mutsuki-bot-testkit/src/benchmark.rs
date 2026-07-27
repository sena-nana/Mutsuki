use mutsuki_bot_protocol::BotEvent;
use mutsuki_plugin_bot_adapter_qqbot::{GatewayFrame, adapter::qq_gateway_frame_to_bot_event};
use serde_json::{Value, json};

pub const BENCHMARK_FIXTURE_VERSION: &str = "mutsuki.bot.benchmark-fixtures/v1";
pub const BENCHMARK_FIXED_SEED: u64 = 1_297_435_713;

pub fn benchmark_gateway_frame(index: usize, adapter_count: usize, command_hit: bool) -> Value {
    let adapter = index % adapter_count.max(1);
    let content = if command_hit {
        format!("/echo fixture-{index:05}")
    } else {
        format!("fixture message {index:05}")
    };
    json!({
        "op": 0,
        "s": index + 1,
        "t": "GROUP_AT_MESSAGE_CREATE",
        "id": format!("benchmark-event-{index:05}"),
        "d": {
            "id": format!("benchmark-message-{index:05}"),
            "group_openid": format!("GROUP_{adapter:02}"),
            "content": format!("<@BOT_{adapter:02}> {content}"),
            "mentions": [{
                "id": format!("BOT_{adapter:02}"),
                "is_you": true,
                "bot": true
            }],
            "time_ms": 1_700_000_000_000_i64 + index as i64,
            "author": {
                "member_openid": format!("USER_{:04}", index % 64),
                "username": "benchmark-user"
            },
            "benchmark": {
                "fixture_version": BENCHMARK_FIXTURE_VERSION,
                "seed": BENCHMARK_FIXED_SEED
            }
        }
    })
}

pub fn benchmark_event(index: usize, adapter_count: usize, command_hit: bool) -> BotEvent {
    let raw = benchmark_gateway_frame(index, adapter_count, command_hit);
    let frame: GatewayFrame = serde_json::from_value(raw).unwrap();
    qq_gateway_frame_to_bot_event(
        &format!("benchmark-adapter-{:02}", index % adapter_count.max(1)),
        frame,
    )
    .expect("benchmark gateway frame maps to a Bot event")
}

pub fn benchmark_card_payload() -> String {
    json!({
        "fixture_version": BENCHMARK_FIXTURE_VERSION,
        "seed": BENCHMARK_FIXED_SEED,
        "meta": "{\"jumpUrl\":\"https://b23.tv/fixed\"}",
        "detail": {
            "url": "https://www.bilibili.com/video/BV1FIXED",
            "description": "same https://b23.tv/fixed and https://www.mihuashi.com/profiles/1"
        },
        "plain": "https://www.bilibili.com/opus/123456"
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_event_fixture_is_stable_and_uses_real_qq_mapping() {
        let first = benchmark_event(7, 4, true);
        let repeated = benchmark_event(7, 4, true);
        assert_eq!(first, repeated);
        assert_eq!(first.bot.account_id, "benchmark-adapter-03");
        assert_eq!(first.message.unwrap().plain_text(), "/echo fixture-00007");
    }
}
