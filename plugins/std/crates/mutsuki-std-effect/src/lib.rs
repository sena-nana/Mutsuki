//! Shared Effect-facade helpers for standard plugins.
//!
//! Public protocol runners validate input, then derive an Effect Task that shares
//! the source payload `Arc` and optionally emits a queued diagnostic event.

use mutsuki_runtime_contracts::{DomainEvent, RunnerResult, Task};
use serde_json::json;

/// Whether facade derivation should emit a queued diagnostic DomainEvent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EffectObservation {
    /// Skip queued-event ID/kind/payload allocation.
    #[default]
    Quiet,
    /// Emit a structured queued event for detailed observation.
    Detailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectDeriveOptions {
    pub observation: EffectObservation,
    /// Static audit kind used when observation is detailed.
    pub queued_event_kind: &'static str,
}

impl Default for EffectDeriveOptions {
    fn default() -> Self {
        Self {
            observation: EffectObservation::Quiet,
            queued_event_kind: "mutsuki.effect.queued",
        }
    }
}

/// One public protocol to Effect protocol mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolPair {
    pub public: &'static str,
    pub effect: &'static str,
    /// Stable audit classification for queued diagnostics.
    pub queued_event_kind: &'static str,
}

/// Single declaration source for public/effect protocol pairs.
#[derive(Clone, Copy, Debug)]
pub struct ProtocolPairTable {
    pairs: &'static [ProtocolPair],
}

impl ProtocolPairTable {
    pub const fn new(pairs: &'static [ProtocolPair]) -> Self {
        Self { pairs }
    }

    pub fn pairs(&self) -> &'static [ProtocolPair] {
        self.pairs
    }

    pub fn public_protocols(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.pairs.iter().map(|pair| pair.public)
    }

    pub fn effect_protocols(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.pairs.iter().map(|pair| pair.effect)
    }

    pub fn effect_for(&self, public: &str) -> Option<&'static str> {
        self.pairs
            .iter()
            .find(|pair| pair.public == public)
            .map(|pair| pair.effect)
    }

    pub fn public_for(&self, effect: &str) -> Option<&'static str> {
        self.pairs
            .iter()
            .find(|pair| pair.effect == effect)
            .map(|pair| pair.public)
    }

    pub fn pair_for_public(&self, public: &str) -> Option<&'static ProtocolPair> {
        self.pairs.iter().find(|pair| pair.public == public)
    }

    /// Detects duplicate public or effect protocol ids.
    pub fn validate_unique(&self) -> Result<(), String> {
        for (index, pair) in self.pairs.iter().enumerate() {
            for other in self.pairs.iter().skip(index + 1) {
                if pair.public == other.public {
                    return Err(format!("duplicate public protocol `{}`", pair.public));
                }
                if pair.effect == other.effect {
                    return Err(format!("duplicate effect protocol `{}`", pair.effect));
                }
            }
        }
        Ok(())
    }
}

/// Derive an Effect Task that shares the source payload Arc.
pub fn derive_effect_task(
    source: &Task,
    effect_protocol: impl Into<String>,
    runner_hint: impl Into<String>,
    options: EffectDeriveOptions,
) -> RunnerResult {
    let effect_task_id = format!("{}:effect", source.task_id);
    let mut effect_task = source.derive_with_protocol(effect_task_id.clone(), effect_protocol);
    effect_task.runner_hint = Some(runner_hint.into());

    let mut result = RunnerResult::completed(source.task_id.clone());
    if options.observation == EffectObservation::Detailed {
        result.events.push(DomainEvent {
            event_id: format!("event:{}:queued", source.task_id),
            kind: options.queued_event_kind.into(),
            payload: json!({ "effect_task_id": effect_task_id }),
        });
    }
    result.tasks.push(effect_task);
    result
}

/// Derive using a protocol pair table entry.
pub fn derive_effect_from_pair(
    source: &Task,
    table: &ProtocolPairTable,
    runner_hint: impl Into<String>,
    observation: EffectObservation,
) -> Result<RunnerResult, String> {
    let pair = table
        .pair_for_public(&source.protocol_id)
        .ok_or_else(|| format!("unsupported public protocol `{}`", source.protocol_id))?;
    Ok(derive_effect_task(
        source,
        pair.effect,
        runner_hint,
        EffectDeriveOptions {
            observation,
            queued_event_kind: pair.queued_event_kind,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PAIRS: &[ProtocolPair] = &[
        ProtocolPair {
            public: "mutsuki.fs.read",
            effect: "effect.mutsuki.fs.read",
            queued_event_kind: "mutsuki.effect.fs.queued",
        },
        ProtocolPair {
            public: "mutsuki.fs.write",
            effect: "effect.mutsuki.fs.write",
            queued_event_kind: "mutsuki.effect.fs.queued",
        },
    ];

    #[test]
    fn protocol_table_rejects_duplicates() {
        const DUP: &[ProtocolPair] = &[
            ProtocolPair {
                public: "a",
                effect: "effect.a",
                queued_event_kind: "k",
            },
            ProtocolPair {
                public: "a",
                effect: "effect.b",
                queued_event_kind: "k",
            },
        ];
        assert!(ProtocolPairTable::new(DUP).validate_unique().is_err());
        assert!(ProtocolPairTable::new(PAIRS).validate_unique().is_ok());
    }

    #[test]
    fn quiet_derive_shares_payload_and_skips_queued_event() {
        let source = Task::new(
            "task-1",
            "mutsuki.fs.read",
            json!({"path": "/tmp/x", "content": "y".repeat(4096)}),
        );
        let result = derive_effect_from_pair(
            &source,
            &ProtocolPairTable::new(PAIRS),
            "effect.runner",
            EffectObservation::Quiet,
        )
        .unwrap();
        assert!(result.events.is_empty());
        assert_eq!(result.tasks.len(), 1);
        assert_eq!(result.tasks[0].protocol_id, "effect.mutsuki.fs.read");
        assert_eq!(source.payload.strong_count(), 2);
        assert_eq!(result.tasks[0].payload.strong_count(), 2);
    }

    #[test]
    fn detailed_derive_emits_static_queued_kind() {
        let source = Task::new("task-2", "mutsuki.fs.write", json!({"path": "/tmp/y"}));
        let result = derive_effect_from_pair(
            &source,
            &ProtocolPairTable::new(PAIRS),
            "effect.runner",
            EffectObservation::Detailed,
        )
        .unwrap();
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].kind, "mutsuki.effect.fs.queued");
        assert!(result.events[0].event_id.contains("queued"));
    }
}
