use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mutsuki_agent_protocol::{
    AgentError, AgentMemoryActivateRequest, AgentMemoryQueryRequest, AgentMemoryQueryResult,
    AgentMemoryRecord, AgentMemoryWriteRequest, AgentResult,
};
use mutsuki_agent_sdk::{memory_cell_ref, memory_resource_ref};

use crate::PLUGIN_ID;

#[derive(Clone, Default)]
pub struct MemoryRouter {
    inner: Arc<MemoryRouterInner>,
}

#[derive(Default)]
struct MemoryRouterInner {
    next_id: AtomicU64,
    records: Mutex<BTreeMap<String, AgentMemoryRecord>>,
    active: Mutex<BTreeSet<String>>,
}

impl MemoryRouter {
    pub fn query(&self, request: AgentMemoryQueryRequest) -> AgentResult<AgentMemoryQueryResult> {
        let query = request.query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(AgentMemoryQueryResult::default());
        }

        let tag_filter: BTreeSet<_> = request.tags.into_iter().collect();
        let mut records: Vec<_> = self
            .inner
            .records
            .lock()
            .expect("memory router mutex poisoned")
            .values()
            .filter(|record| {
                tag_filter.is_empty() || record.tags.iter().any(|tag| tag_filter.contains(tag))
            })
            .filter_map(|record| {
                let text = record.text.to_lowercase();
                let score = if text.contains(&query) {
                    1.0
                } else {
                    query
                        .split_whitespace()
                        .filter(|term| text.contains(term))
                        .count() as f32
                        / query.split_whitespace().count().max(1) as f32
                };
                (score > 0.0).then(|| {
                    let mut record = record.clone();
                    record.score = score;
                    record
                })
            })
            .collect();
        records.sort_by(|left, right| right.score.total_cmp(&left.score));
        records.truncate(request.limit.max(1));
        Ok(AgentMemoryQueryResult { records })
    }

    pub fn write(&self, request: AgentMemoryWriteRequest) -> AgentResult<AgentMemoryRecord> {
        if request.text.trim().is_empty() {
            return Err(AgentError::invalid_input("memory text is required"));
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let memory_id = format!("agent-memory-{id}");
        let record = AgentMemoryRecord {
            memory_id: memory_id.clone(),
            text: request.text,
            tags: request.tags,
            score: 1.0,
            metadata: request.metadata,
            resource: Some(memory_resource_ref(PLUGIN_ID, &memory_id)),
            cell: Some(memory_cell_ref(PLUGIN_ID, &memory_id)),
        };
        self.inner
            .records
            .lock()
            .expect("memory router mutex poisoned")
            .insert(record.memory_id.clone(), record.clone());
        Ok(record)
    }

    pub fn activate(&self, request: AgentMemoryActivateRequest) -> AgentResult<AgentMemoryRecord> {
        let record = self
            .inner
            .records
            .lock()
            .expect("memory router mutex poisoned")
            .get(&request.memory_id)
            .cloned()
            .ok_or_else(|| {
                AgentError::not_found(format!("memory `{}` not found", request.memory_id))
            })?;
        self.inner
            .active
            .lock()
            .expect("memory router mutex poisoned")
            .insert(record.memory_id.clone());
        Ok(record)
    }
}
