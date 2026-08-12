use std::ops::Range;

use mutsuki_agent_contracts::{AgentMessage, AgentRole};

const SUMMARY_STRATEGY: &str = "deterministic_turn_window_v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptContextDisposition {
    Unchanged,
    Compacted { dropped_messages: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedTranscriptContext {
    pub messages: Vec<AgentMessage>,
    pub disposition: TranscriptContextDisposition,
    pub estimated_tokens: u64,
    pub budget_satisfied: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TranscriptContextWindow;

impl TranscriptContextWindow {
    pub fn prepare(
        &self,
        messages: &[AgentMessage],
        max_context_tokens: Option<u64>,
    ) -> PreparedTranscriptContext {
        let estimated_tokens = estimate_messages_tokens(messages);
        let Some(limit) = max_context_tokens.filter(|limit| *limit > 0) else {
            return unchanged(messages, estimated_tokens, true);
        };
        if estimated_tokens <= limit {
            return unchanged(messages, estimated_tokens, true);
        }

        let system_prefix_len = messages
            .iter()
            .take_while(|message| message.role == AgentRole::System)
            .count();
        let system_messages = messages[..system_prefix_len].to_vec();
        let turns = conversation_turns(messages, system_prefix_len);
        if turns.is_empty() {
            return unchanged(messages, estimated_tokens, false);
        }

        let system_tokens = estimate_messages_tokens(&system_messages);
        let summary_reserve = (limit / 5).clamp(32, 512);
        let tail_budget = limit.saturating_sub(system_tokens.saturating_add(summary_reserve));
        let mut kept_start = turns.len() - 1;
        let mut kept_tokens = estimate_messages_tokens(&messages[turns[kept_start].clone()]);
        while kept_start > 0 {
            let candidate = &turns[kept_start - 1];
            let candidate_tokens = estimate_messages_tokens(&messages[candidate.clone()]);
            if kept_tokens.saturating_add(candidate_tokens) > tail_budget {
                break;
            }
            kept_start -= 1;
            kept_tokens = kept_tokens.saturating_add(candidate_tokens);
        }

        let dropped_ranges = &turns[..kept_start];
        let dropped_messages = dropped_ranges.iter().map(Range::len).sum::<usize>();
        if dropped_messages == 0 {
            return unchanged(messages, estimated_tokens, false);
        }

        let mut prepared = system_messages;
        let available_summary_tokens = limit
            .saturating_sub(estimate_messages_tokens(&prepared))
            .saturating_sub(kept_tokens);
        if available_summary_tokens > 0 {
            prepared.push(compaction_summary(
                messages,
                dropped_ranges,
                dropped_messages,
                available_summary_tokens,
            ));
        }
        for range in &turns[kept_start..] {
            prepared.extend_from_slice(&messages[range.clone()]);
        }
        let estimated_tokens = estimate_messages_tokens(&prepared);
        PreparedTranscriptContext {
            messages: prepared,
            disposition: TranscriptContextDisposition::Compacted { dropped_messages },
            estimated_tokens,
            budget_satisfied: estimated_tokens <= limit,
        }
    }
}

pub fn estimate_messages_tokens(messages: &[AgentMessage]) -> u64 {
    messages
        .iter()
        .map(|message| {
            serde_json::to_vec(message)
                .map(|bytes| (bytes.len() as u64).div_ceil(4).max(1))
                .unwrap_or_else(|_| (message.content.len() as u64).div_ceil(4).max(1))
        })
        .sum()
}

fn unchanged(
    messages: &[AgentMessage],
    estimated_tokens: u64,
    budget_satisfied: bool,
) -> PreparedTranscriptContext {
    PreparedTranscriptContext {
        messages: messages.to_vec(),
        disposition: TranscriptContextDisposition::Unchanged,
        estimated_tokens,
        budget_satisfied,
    }
}

fn conversation_turns(messages: &[AgentMessage], system_prefix_len: usize) -> Vec<Range<usize>> {
    let mut turns = Vec::new();
    let mut start = system_prefix_len;
    let mut contains_user = false;
    for (index, message) in messages.iter().enumerate().skip(system_prefix_len) {
        if message.role == AgentRole::System && index > start {
            turns.push(start..index);
            start = index;
            contains_user = false;
        } else if message.role == AgentRole::User && contains_user {
            turns.push(start..index);
            start = index;
        }
        contains_user |= message.role == AgentRole::User;
    }
    if start < messages.len() {
        turns.push(start..messages.len());
    }
    turns.retain(|range| {
        messages[range.clone()]
            .iter()
            .any(|message| message.role != AgentRole::System)
    });
    turns
}

fn compaction_summary(
    messages: &[AgentMessage],
    ranges: &[Range<usize>],
    dropped_messages: usize,
    token_budget: u64,
) -> AgentMessage {
    let char_budget = token_budget.saturating_mul(4).clamp(96, 8_192) as usize;
    let mut content = format!(
        "Earlier conversation was compacted into a deterministic history digest ({dropped_messages} messages).\n"
    );
    'turns: for (turn_index, range) in ranges.iter().enumerate() {
        for message in messages[range.clone()]
            .iter()
            .filter(|message| matches!(message.role, AgentRole::User | AgentRole::Assistant))
        {
            let role = match message.role {
                AgentRole::User => "user",
                AgentRole::Assistant => "assistant",
                _ => continue,
            };
            let normalized = message
                .content
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let entry = format!("- turn {} {role}: {normalized}\n", turn_index + 1);
            if content.len().saturating_add(entry.len()) > char_budget {
                let remaining = char_budget.saturating_sub(content.len());
                if remaining > 24 {
                    content.extend(entry.chars().take(remaining.saturating_sub(1)));
                    content.push('…');
                }
                break 'turns;
            }
            content.push_str(&entry);
        }
    }
    let mut summary = AgentMessage::system(content);
    summary.metadata = Some(serde_json::json!({
        "context_compaction": {
            "strategy": SUMMARY_STRATEGY,
            "dropped_messages": dropped_messages,
        }
    }));
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_message(role: AgentRole, marker: &str) -> AgentMessage {
        AgentMessage {
            role,
            content: format!("{marker} {}", "context ".repeat(320)),
            name: None,
            metadata: None,
            parts: Vec::new(),
        }
    }

    #[test]
    fn leaves_transcript_unchanged_inside_budget() {
        let messages = vec![
            AgentMessage::user("hello"),
            AgentMessage::assistant("world"),
        ];

        let prepared = TranscriptContextWindow.prepare(&messages, Some(1_000));

        assert_eq!(prepared.messages, messages);
        assert_eq!(
            prepared.disposition,
            TranscriptContextDisposition::Unchanged
        );
        assert!(prepared.budget_satisfied);
    }

    #[test]
    fn compacts_old_turns_and_preserves_system_and_latest_turn() {
        let messages = vec![
            AgentMessage::system("product instructions"),
            long_message(AgentRole::User, "old-user"),
            long_message(AgentRole::Assistant, "old-assistant"),
            AgentMessage::user("latest-user"),
            AgentMessage::assistant("latest-assistant"),
        ];

        let prepared = TranscriptContextWindow.prepare(&messages, Some(300));

        assert!(matches!(
            prepared.disposition,
            TranscriptContextDisposition::Compacted {
                dropped_messages: 2
            }
        ));
        assert!(prepared.messages.iter().any(|message| {
            message.role == AgentRole::System && message.content == "product instructions"
        }));
        assert!(
            prepared
                .messages
                .iter()
                .any(|message| message.content == "latest-user")
        );
        assert!(prepared.messages.iter().any(|message| {
            message
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("context_compaction"))
                .is_some()
        }));
        assert!(
            !prepared
                .messages
                .iter()
                .any(|message| message.content.starts_with("old-user context"))
        );
    }

    #[test]
    fn keeps_latest_tool_causal_chain_as_one_turn() {
        let mut assistant = AgentMessage::assistant("");
        assistant.metadata = Some(serde_json::json!({
            "tool_calls": [{"call_id": "call-1", "name": "read", "arguments": {}}]
        }));
        let tool = AgentMessage {
            role: AgentRole::Tool,
            content: "tool-output".into(),
            name: Some("read".into()),
            metadata: Some(serde_json::json!({"call_id": "call-1"})),
            parts: Vec::new(),
        };
        let messages = vec![
            long_message(AgentRole::User, "old"),
            long_message(AgentRole::Assistant, "old-result"),
            AgentMessage::user("latest"),
            assistant.clone(),
            tool.clone(),
        ];

        let prepared = TranscriptContextWindow.prepare(&messages, Some(220));

        assert!(prepared.messages.contains(&assistant));
        assert!(prepared.messages.contains(&tool));
    }

    #[test]
    fn turn_scoped_system_context_is_compacted_with_its_old_turn() {
        let old_context = AgentMessage::system(format!(
            "Product turn context old-marker {}",
            "workspace ".repeat(400)
        ));
        let latest_context = AgentMessage::system("Product turn context latest-marker");
        let messages = vec![
            AgentMessage::system("global instructions"),
            AgentMessage::user("bootstrap"),
            AgentMessage::assistant("bootstrap complete"),
            old_context.clone(),
            long_message(AgentRole::User, "old"),
            long_message(AgentRole::Assistant, "old-result"),
            latest_context.clone(),
            AgentMessage::user("latest"),
        ];

        let prepared = TranscriptContextWindow.prepare(&messages, Some(260));

        assert!(!prepared.messages.contains(&old_context));
        assert!(prepared.messages.contains(&latest_context));
        assert_eq!(prepared.messages[0].content, "global instructions");
    }
}
