use std::sync::Mutex;

use mutsuki_agent_contracts::{AgentError, AgentSessionCheckpoint};

use crate::{CoordinatorFence, CoordinatorLease};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentRecoveryMode {
    Active,
    DegradedReadOnly { reason: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRecoveredSession {
    pub checkpoint: AgentSessionCheckpoint,
    pub mode: AgentRecoveryMode,
    pub coordinator: Option<CoordinatorLease>,
}

impl AgentRecoveredSession {
    pub fn authorize_side_effect(&self, now_unix_ms: u64) -> Result<(), AgentError> {
        match (&self.mode, &self.coordinator) {
            (AgentRecoveryMode::Active, Some(lease)) if now_unix_ms < lease.expires_at_unix_ms => {
                Ok(())
            }
            (AgentRecoveryMode::Active, Some(_)) => Err(AgentError::new(
                "agent.coordinator.lease_expired",
                "recovered coordinator lease has expired",
            )),
            (AgentRecoveryMode::Active, None) => Err(AgentError::new(
                "agent.coordinator.lease_missing",
                "active recovery requires a coordinator lease",
            )),
            (AgentRecoveryMode::DegradedReadOnly { reason }, _) => Err(AgentError::new(
                "agent.recovery.degraded_read_only",
                reason.clone(),
            )),
        }
    }
}

#[derive(Default)]
pub struct AgentSessionRecoveryCoordinator {
    fence: Mutex<CoordinatorFence>,
}

impl AgentSessionRecoveryCoordinator {
    pub fn recover(
        &self,
        mut checkpoint: AgentSessionCheckpoint,
        previous: Option<CoordinatorLease>,
        candidate: Option<CoordinatorLease>,
        durable_state_available: bool,
        now_unix_ms: u64,
    ) -> Result<AgentRecoveredSession, AgentError> {
        if !durable_state_available {
            let reason =
                "durable session state is unavailable; side effects are disabled".to_owned();
            checkpoint.degraded_reason = Some(reason.clone());
            return Ok(AgentRecoveredSession {
                checkpoint,
                mode: AgentRecoveryMode::DegradedReadOnly { reason },
                coordinator: None,
            });
        }

        let candidate = candidate.ok_or_else(|| {
            AgentError::new(
                "agent.coordinator.lease_missing",
                "durable recovery requires a new coordinator lease",
            )
        })?;
        if candidate.session_id != checkpoint.session_id {
            return Err(AgentError::invalid_input(
                "coordinator lease must belong to the recovered session",
            ));
        }
        if candidate.expires_at_unix_ms <= now_unix_ms {
            return Err(AgentError::new(
                "agent.coordinator.lease_expired",
                "candidate coordinator lease has already expired",
            ));
        }

        let mut fence = self.fence.lock().expect("agent recovery fence mutex");
        if let Some(previous) = previous {
            fence.restore(previous)?;
        }
        fence.acquire(candidate.clone())?;
        fence.validate(
            &candidate.session_id,
            candidate.epoch,
            &candidate.fencing_token,
            now_unix_ms,
        )?;
        checkpoint.degraded_reason = None;
        Ok(AgentRecoveredSession {
            checkpoint,
            mode: AgentRecoveryMode::Active,
            coordinator: Some(candidate),
        })
    }

    pub fn renew(&self, lease: CoordinatorLease) -> Result<(), AgentError> {
        self.fence
            .lock()
            .expect("agent recovery fence mutex")
            .renew(lease)
    }

    pub fn validate(
        &self,
        session_id: &str,
        epoch: u64,
        fencing_token: &str,
        now_unix_ms: u64,
    ) -> Result<(), AgentError> {
        self.fence
            .lock()
            .expect("agent recovery fence mutex")
            .validate(session_id, epoch, fencing_token, now_unix_ms)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mutsuki_agent_contracts::{AgentBudget, ResourceRef, SessionSnapshotRef, SessionVersion};
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };

    use super::*;

    fn resource() -> ResourceRef {
        ResourceRef {
            resource_id: ResourceId {
                kind_id: "agent.session".into(),
                slot_id: "session".into(),
                generation: 1,
                version: 1,
            },
            ref_id: "resource:session".into(),
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "agent.store".into(),
            resource_kind: "agent.session".into(),
            schema: "mutsuki.agent.session@1".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: None,
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    fn lease(epoch: u64, token: &str, expires: u64) -> CoordinatorLease {
        CoordinatorLease {
            session_id: "session".into(),
            node_id: format!("node-{epoch}"),
            epoch,
            fencing_token: token.into(),
            expires_at_unix_ms: expires,
        }
    }

    fn checkpoint() -> AgentSessionCheckpoint {
        let resource = resource();
        AgentSessionCheckpoint {
            session_id: "session".into(),
            profile_id: "coding".into(),
            version: SessionVersion(1),
            budget: AgentBudget::default(),
            state: resource.clone(),
            snapshot: SessionSnapshotRef {
                session_id: "session".into(),
                version: SessionVersion(1),
                snapshot: resource,
                base: None,
                deltas: Vec::new(),
            },
            pending_approvals: Vec::new(),
            plugin_generations: BTreeMap::new(),
            attempts: BTreeMap::new(),
            degraded_reason: None,
        }
    }

    #[test]
    fn unavailable_durable_state_is_read_only_and_new_epoch_fences_old_owner() {
        let coordinator = AgentSessionRecoveryCoordinator::default();
        let previous = Some(lease(3, "old", 100));
        let degraded = coordinator
            .recover(
                checkpoint(),
                previous.clone(),
                Some(lease(4, "new", 200)),
                false,
                10,
            )
            .unwrap();
        assert!(matches!(
            degraded.mode,
            AgentRecoveryMode::DegradedReadOnly { .. }
        ));
        assert_eq!(
            degraded.authorize_side_effect(10).unwrap_err().code,
            "agent.recovery.degraded_read_only"
        );

        assert_eq!(
            coordinator
                .recover(
                    checkpoint(),
                    previous.clone(),
                    Some(lease(3, "stale", 200)),
                    true,
                    10
                )
                .unwrap_err()
                .code,
            "agent.coordinator.epoch_conflict"
        );
        let recovered = coordinator
            .recover(checkpoint(), previous, Some(lease(4, "new", 200)), true, 10)
            .unwrap();
        recovered.authorize_side_effect(10).unwrap();
        coordinator.validate("session", 4, "new", 10).unwrap();
        assert_eq!(
            coordinator
                .validate("session", 3, "old", 10)
                .unwrap_err()
                .code,
            "agent.coordinator.fenced"
        );
    }
}
