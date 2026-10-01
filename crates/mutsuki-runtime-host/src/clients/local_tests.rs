use super::*;
use mutsuki_runtime_contracts::{ERR_REGISTRY_UNAUTHORIZED, ResourceDescriptorInvalidation};
use mutsuki_runtime_sdk::{ResourceProviderOrdering, ResourceProviderOutcome};

struct TestProvider {
    invalidates: bool,
    fails: bool,
}

impl ResourceProviderGateway for TestProvider {
    fn execute(&self, _request: Request) -> ResourceProviderOutcome<Reply> {
        ResourceProviderOutcome {
            result: if self.fails {
                Err(crate::error::host_failure(
                    "test.provider.failure",
                    "failed",
                ))
            } else {
                Ok(Reply::Bytes(vec![42]))
            },
            invalidations: self
                .invalidates
                .then(|| ResourceDescriptorInvalidation {
                    provider_id: "memory".into(),
                    ref_id: "resource-0".into(),
                    generation: 1,
                })
                .into_iter()
                .collect(),
        }
    }
}

fn request() -> Request {
    Request::CreateCapability {
        kind_id: "test".into(),
        schema: "test.v1".into(),
    }
}

fn client(invalidates: bool, fails: bool) -> LocalResourceClient {
    LocalResourceClient::with_provider("memory", TestProvider { invalidates, fails })
}

#[test]
fn standalone_client_rejects_invalidations_independent_of_result() {
    for fails in [false, true] {
        let error = client(true, fails)
            .execute("memory", request())
            .unwrap_err();
        assert_eq!(error.error().code, ERR_REGISTRY_UNAUTHORIZED);
        assert_eq!(
            error.error().route,
            "host.plugin.resource_provider_unsupported"
        );
    }
}

#[test]
fn standalone_client_preserves_non_lifecycle_results() {
    assert_eq!(
        client(false, false).execute("memory", request()).unwrap(),
        Reply::Bytes(vec![42])
    );
    assert_eq!(
        client(false, true)
            .execute("memory", request())
            .unwrap_err()
            .error()
            .route,
        "test.provider.failure"
    );
}

#[test]
fn standalone_client_rejects_ordered_provider_before_execution() {
    struct Ordered;
    impl ResourceProviderGateway for Ordered {
        fn execute(&self, _request: Request) -> ResourceProviderOutcome<Reply> {
            panic!("ordered provider must be rejected before execution")
        }
        fn ordering(&self) -> ResourceProviderOrdering {
            ResourceProviderOrdering::Ordered
        }
    }
    let client = LocalResourceClient::with_provider("memory", Ordered);
    assert!(client.execute("memory", request()).is_err());
}
