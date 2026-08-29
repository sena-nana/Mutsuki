use std::collections::BTreeMap;

use mutsuki_agent_contracts::AgentError;

use crate::CoordinatorLease;

#[derive(Default)]
pub struct CoordinatorFence {
    active: BTreeMap<String, CoordinatorLease>,
}

impl CoordinatorFence {
    pub fn acquire(&mut self, lease: CoordinatorLease) -> Result<(), AgentError> {
        if lease.session_id.trim().is_empty()
            || lease.node_id.trim().is_empty()
            || lease.fencing_token.trim().is_empty()
            || lease.epoch == 0
        {
            return Err(AgentError::invalid_input(
                "coordinator lease identity, epoch and fencing token are required",
            ));
        }
        if let Some(current) = self.active.get(&lease.session_id)
            && lease.epoch <= current.epoch
        {
            return Err(AgentError::new(
                "agent.coordinator.epoch_conflict",
                "coordinator epoch must increase on takeover",
            ));
        }
        self.active.insert(lease.session_id.clone(), lease);
        Ok(())
    }

    pub fn restore(&mut self, lease: CoordinatorLease) -> Result<(), AgentError> {
        if let Some(current) = self.active.get(&lease.session_id) {
            if current.epoch > lease.epoch
                || (current.epoch == lease.epoch && current.fencing_token != lease.fencing_token)
            {
                return Err(AgentError::new(
                    "agent.coordinator.fenced",
                    "persisted coordinator lease is older than the active fence",
                ));
            }
            if current.epoch == lease.epoch {
                return Ok(());
            }
        }
        self.acquire(lease)
    }

    pub fn renew(&mut self, lease: CoordinatorLease) -> Result<(), AgentError> {
        let current = self
            .active
            .get(&lease.session_id)
            .ok_or_else(|| AgentError::not_found("coordinator lease was not found"))?;
        if current.epoch != lease.epoch
            || current.node_id != lease.node_id
            || current.fencing_token != lease.fencing_token
        {
            return Err(AgentError::new(
                "agent.coordinator.fenced",
                "only the active coordinator may renew its lease",
            ));
        }
        if lease.expires_at_unix_ms <= current.expires_at_unix_ms {
            return Err(AgentError::invalid_input(
                "coordinator renewal must extend the lease",
            ));
        }
        self.active.insert(lease.session_id.clone(), lease);
        Ok(())
    }

    pub fn current(&self, session_id: &str) -> Option<&CoordinatorLease> {
        self.active.get(session_id)
    }

    pub fn validate(
        &self,
        session_id: &str,
        epoch: u64,
        fencing_token: &str,
        now_unix_ms: u64,
    ) -> Result<(), AgentError> {
        let lease = self
            .active
            .get(session_id)
            .ok_or_else(|| AgentError::not_found("coordinator lease was not found"))?;
        if lease.epoch != epoch || lease.fencing_token != fencing_token {
            return Err(AgentError::new(
                "agent.coordinator.fenced",
                "coordinator epoch or fencing token is stale",
            ));
        }
        if now_unix_ms >= lease.expires_at_unix_ms {
            return Err(AgentError::new(
                "agent.coordinator.lease_expired",
                "coordinator lease has expired",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(node: &str, epoch: u64, token: &str) -> CoordinatorLease {
        CoordinatorLease {
            session_id: "session".into(),
            node_id: node.into(),
            epoch,
            fencing_token: token.into(),
            expires_at_unix_ms: 100,
        }
    }

    #[test]
    fn takeover_fences_the_old_coordinator() {
        let mut fence = CoordinatorFence::default();
        fence.acquire(lease("node-a", 1, "token-a")).unwrap();
        fence.acquire(lease("node-b", 2, "token-b")).unwrap();
        assert_eq!(
            fence
                .validate("session", 1, "token-a", 10)
                .unwrap_err()
                .code,
            "agent.coordinator.fenced"
        );
        fence.validate("session", 2, "token-b", 10).unwrap();
    }

    #[test]
    fn persisted_lease_restores_idempotently_and_only_owner_can_renew() {
        let mut fence = CoordinatorFence::default();
        fence.restore(lease("node-a", 1, "token-a")).unwrap();
        fence.restore(lease("node-a", 1, "token-a")).unwrap();
        assert_eq!(fence.current("session").unwrap().epoch, 1);

        let mut renewed = lease("node-a", 1, "token-a");
        renewed.expires_at_unix_ms = 200;
        fence.renew(renewed).unwrap();
        assert_eq!(fence.current("session").unwrap().expires_at_unix_ms, 200);

        let mut stale_owner = lease("node-b", 1, "token-b");
        stale_owner.expires_at_unix_ms = 300;
        assert_eq!(
            fence.renew(stale_owner).unwrap_err().code,
            "agent.coordinator.fenced"
        );
    }
}
