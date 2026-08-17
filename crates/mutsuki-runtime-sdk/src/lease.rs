use mutsuki_runtime_contracts::{ExclusiveWriteLease, LeaseToken, ResourceLease};
use mutsuki_runtime_core::RuntimeResult;

/// Exclusive write lease. Not `Clone` / `Copy`. Drop does not notify Core.
#[derive(Debug)]
pub struct ExclusiveWriteGuard {
    lease: ExclusiveWriteLease,
}

impl ExclusiveWriteGuard {
    pub fn new(lease: ExclusiveWriteLease) -> Self {
        Self { lease }
    }

    pub fn as_lease(&self) -> &ExclusiveWriteLease {
        &self.lease
    }

    pub fn token(&self) -> &LeaseToken {
        &self.lease.token
    }

    pub fn release<F>(self, releaser: F) -> RuntimeResult<()>
    where
        F: FnOnce(&ExclusiveWriteLease) -> RuntimeResult<()>,
    {
        releaser(&self.lease)
    }
}

/// Exclusive cell lease. Not `Clone` / `Copy`. Drop does not notify Core.
#[derive(Debug)]
pub struct ResourceLeaseGuard {
    lease: ResourceLease,
}

impl ResourceLeaseGuard {
    pub fn new(lease: ResourceLease) -> Self {
        Self { lease }
    }

    pub fn as_lease(&self) -> &ResourceLease {
        &self.lease
    }

    pub fn release<F>(self, releaser: F) -> RuntimeResult<()>
    where
        F: FnOnce(&ResourceLease) -> RuntimeResult<()>,
    {
        releaser(&self.lease)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use mutsuki_runtime_contracts::{ExclusiveWriteLease, LeaseToken, ResourceLease, RuntimeError};
    use mutsuki_runtime_core::RuntimeFailure;

    use super::{ExclusiveWriteGuard, ResourceLeaseGuard};

    #[test]
    fn write_guard_release_consumes_self_and_invokes_releaser_once() {
        let lease = sample_write_lease("lease-token-1");
        let released = Cell::new(false);
        let guard = ExclusiveWriteGuard::new(lease.clone());

        assert_eq!(guard.as_lease(), &lease);
        assert_eq!(guard.token(), &lease.token);
        guard
            .release(|inner| {
                assert!(!released.replace(true));
                assert_eq!(inner, &lease);
                Ok(())
            })
            .expect("release should succeed");
        assert!(released.get());
    }

    #[test]
    fn write_guard_release_propagates_releaser_error() {
        let error = ExclusiveWriteGuard::new(sample_write_lease("lease-token-1"))
            .release(|_| {
                Err(RuntimeFailure::new(RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_RESOURCE_LEASE_EXPIRED,
                    "runtime.resource_manager",
                    "resource.write_lease.release.resource:state",
                )))
            })
            .expect_err("releaser error should surface");

        assert_eq!(
            error.error().code,
            mutsuki_runtime_contracts::ERR_RESOURCE_LEASE_EXPIRED
        );
    }

    #[test]
    fn resource_lease_guard_release_consumes_self() {
        let lease = sample_resource_lease("cell-lease-1");
        let released = Cell::new(false);
        let guard = ResourceLeaseGuard::new(lease.clone());

        guard
            .release(|inner| {
                assert!(!released.replace(true));
                assert_eq!(inner.lease_id, lease.lease_id);
                Ok(())
            })
            .expect("release should succeed");
        assert!(released.get());
    }

    #[test]
    fn lease_guards_are_not_clone_or_copy() {
        assert_not_impl_clone::<ExclusiveWriteGuard>();
        assert_not_impl_clone::<ResourceLeaseGuard>();
    }

    fn sample_write_lease(token_id: &str) -> ExclusiveWriteLease {
        ExclusiveWriteLease {
            token: LeaseToken {
                token_id: token_id.into(),
                ref_id: "resource:state".into(),
                owner: "runner-a".into(),
                mode: "exclusive_write".into(),
                expires_at_step: Some(5),
                generation: 1,
            },
        }
    }

    fn sample_resource_lease(lease_id: &str) -> ResourceLease {
        ResourceLease {
            lease_id: lease_id.into(),
            cell_id: "cell-1".into(),
            borrower_task_id: "task-1".into(),
            borrower_executor_id: "executor-1".into(),
            mode: "exclusive".into(),
            expires_at_step: Some(5),
            generation: 1,
        }
    }

    trait AmbiguousIfImpl<A> {
        fn some_item() {}
    }

    impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
    impl<T: ?Sized + Clone> AmbiguousIfImpl<u8> for T {}

    fn assert_not_impl_clone<T: ?Sized>() {
        <T as AmbiguousIfImpl<_>>::some_item();
    }
}
