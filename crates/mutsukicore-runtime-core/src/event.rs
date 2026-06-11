use std::collections::BTreeMap;

use mutsukicore_runtime_contracts::{
    AgentId, RuntimeError, RuntimeEvent, RuntimeEventKind, ScalarValue,
};

#[derive(Clone, Debug)]
pub(crate) struct EventDraft {
    pub(crate) kind: RuntimeEventKind,
    pub(crate) name: String,
    pub(crate) agent_id: Option<AgentId>,
    pub(crate) attributes: BTreeMap<String, ScalarValue>,
    pub(crate) error: Option<RuntimeError>,
}

#[derive(Clone, Debug, Default)]
pub struct EventBook {
    events: Vec<RuntimeEvent>,
    next_sequence: u64,
}

impl EventBook {
    pub fn record(
        &mut self,
        kind: RuntimeEventKind,
        name: impl Into<String>,
        agent_id: Option<AgentId>,
        attributes: BTreeMap<String, ScalarValue>,
        error: Option<RuntimeError>,
    ) -> RuntimeEvent {
        self.next_sequence += 1;
        let event = RuntimeEvent {
            sequence: self.next_sequence,
            kind,
            name: name.into(),
            agent_id,
            attributes,
            error,
        };
        self.events.push(event.clone());
        event
    }

    pub(crate) fn snapshot_with_drafts(&self, drafts: &[EventDraft]) -> Vec<RuntimeEvent> {
        let mut events = self.events.clone();
        let mut next_sequence = self.next_sequence;
        events.extend(drafts.iter().map(|draft| {
            next_sequence += 1;
            event_from_draft(next_sequence, draft)
        }));
        events
    }

    pub(crate) fn append_drafts(&mut self, drafts: Vec<EventDraft>) {
        for draft in drafts {
            let event = self.next_event(draft);
            self.events.push(event);
        }
    }

    pub fn drain(&mut self) -> Vec<RuntimeEvent> {
        self.events.drain(..).collect()
    }

    fn next_event(&mut self, draft: EventDraft) -> RuntimeEvent {
        self.next_sequence += 1;
        event_from_draft(self.next_sequence, &draft)
    }
}

fn event_from_draft(sequence: u64, draft: &EventDraft) -> RuntimeEvent {
    RuntimeEvent {
        sequence,
        kind: draft.kind.clone(),
        name: draft.name.clone(),
        agent_id: draft.agent_id.clone(),
        attributes: draft.attributes.clone(),
        error: draft.error.clone(),
    }
}
