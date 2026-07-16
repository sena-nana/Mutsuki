use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex};

use crossbeam_channel::Sender;
use mutsuki_runtime_contracts::RuntimeError;
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_wire::WireLimits;

use super::transport_failure;

struct PendingRequest {
    management: bool,
    sender: Sender<RuntimeResult<Vec<u8>>>,
}

#[derive(Default)]
struct PendingState {
    requests: BTreeMap<u64, PendingRequest>,
    failure: Option<RuntimeError>,
    closed: bool,
}

#[derive(Default)]
pub(super) struct PendingShared {
    state: Mutex<PendingState>,
    ready: Condvar,
}

impl PendingShared {
    pub(super) fn insert(
        &self,
        request_id: u64,
        management: bool,
        sender: Sender<RuntimeResult<Vec<u8>>>,
        limits: WireLimits,
    ) -> RuntimeResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| transport_failure("pending table lock poisoned"))?;
        if let Some(error) = state.failure.clone() {
            return Err(RuntimeFailure::new(error));
        }
        if state.closed {
            return Err(transport_failure("transport is closed"));
        }
        if state.requests.len() >= limits.max_in_flight_requests {
            return Err(transport_failure("pending request limit reached"));
        }
        let work_capacity = limits
            .max_in_flight_requests
            .saturating_sub(limits.management_reserved_requests);
        let work_in_flight = state
            .requests
            .values()
            .filter(|request| !request.management)
            .count();
        if !management && work_in_flight >= work_capacity {
            return Err(transport_failure("work pending capacity is exhausted"));
        }
        if state.requests.contains_key(&request_id) {
            return Err(transport_failure("duplicate request id"));
        }
        state
            .requests
            .insert(request_id, PendingRequest { management, sender });
        self.ready.notify_one();
        Ok(())
    }

    pub(super) fn remove(&self, request_id: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.requests.remove(&request_id);
        }
    }

    pub(super) fn take(&self, request_id: u64) -> Option<Sender<RuntimeResult<Vec<u8>>>> {
        self.state
            .lock()
            .ok()?
            .requests
            .remove(&request_id)
            .map(|request| request.sender)
    }

    pub(super) fn wait_until_ready_or_closed(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        while state.requests.is_empty() && !state.closed && state.failure.is_none() {
            let Ok(next) = self.ready.wait(state) else {
                return false;
            };
            state = next;
        }
        !state.closed && state.failure.is_none()
    }

    pub(super) fn fail(&self, error: RuntimeError) {
        let senders = match self.state.lock() {
            Ok(mut state) => {
                if state.failure.is_none() {
                    state.failure = Some(error.clone());
                }
                std::mem::take(&mut state.requests)
            }
            Err(_) => return,
        };
        self.ready.notify_all();
        for (_, request) in senders {
            let _ = request.sender.send(Err(RuntimeFailure::new(error.clone())));
        }
    }

    pub(super) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
        }
        self.ready.notify_all();
    }
}
