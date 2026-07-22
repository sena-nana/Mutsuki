use super::activator::{ProcessAppActivator, TauriAppActivator};
use super::delivery::AppDeliveryService;
use super::draft::DeliveryDraftStore;
use super::endpoint::AppCapabilityEndpoint;
use super::transport::{AppLinkTransport, InMemoryAppLinkTransport, LinkLocalAppTransport};
use super::types::{
    AppDeliveryError, AppDeliveryOptions, AppDescriptor, AppId, AppIdentity, DeliveryPhase,
    HOST_PROTOCOL_VERSION,
};
use mutsuki_runtime_contracts::{CapabilityDescriptor, DeliveryReceipt};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn source_identity() -> AppIdentity {
    AppIdentity {
        app_id: AppId::new("lilia.github").unwrap(),
        instance_id: "source-1".into(),
    }
}

fn capability() -> CapabilityDescriptor {
    CapabilityDescriptor::new("lilia.code.task.accept", 1, 1)
}

fn options_with_id(request_id: &str) -> AppDeliveryOptions {
    AppDeliveryOptions {
        request_id: Some(request_id.into()),
        ..AppDeliveryOptions::default()
    }
}

#[tokio::test]
async fn online_direct_delivery_returns_accepted_receipt() {
    let target = AppId::new("lilia.code").unwrap();
    let transport = InMemoryAppLinkTransport::new();
    transport.register_online(&target, vec![capability()]);
    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        transport,
        DeliveryDraftStore::memory(),
    );

    let receipt = service
        .request_app(
            target,
            capability(),
            json!({"title": "fix CI"}),
            options_with_id("req-online-1"),
        )
        .await
        .unwrap();

    assert!(matches!(
        receipt,
        DeliveryReceipt::Accepted { request_id, .. } if request_id == "req-online-1"
    ));
    assert_eq!(
        service.phase_for("req-online-1"),
        Some(DeliveryPhase::Accepted)
    );
}

#[tokio::test]
async fn offline_activation_waits_for_capability_ready_before_transmit() {
    let target = AppId::new("lilia.code").unwrap();
    let transport = Arc::new(InMemoryAppLinkTransport::new());
    transport.register_offline(&target, vec![capability()], Duration::from_millis(20));

    let activator = ProcessAppActivator::new();
    activator
        .register(AppDescriptor {
            app_id: target.clone(),
            display_name: "LiliaCode".into(),
            executable: None,
            launch_args: Vec::new(),
            bundle_id: None,
        })
        .await;

    struct Activating {
        inner: ProcessAppActivator,
        transport: Arc<InMemoryAppLinkTransport>,
        delay: Duration,
    }
    impl TauriAppActivator for Activating {
        async fn resolve(
            &self,
            app_id: &AppId,
        ) -> Result<AppDescriptor, super::types::ActivationError> {
            self.inner.resolve(app_id).await
        }
        async fn activate(
            &self,
            app: &AppDescriptor,
        ) -> Result<super::types::ActivationReceipt, super::types::ActivationError> {
            tokio::time::sleep(self.delay).await;
            self.transport.mark_online(&app.app_id);
            Ok(super::types::ActivationReceipt {
                app_id: app.app_id.clone(),
                instance_id: "activated-1".into(),
                already_running: false,
            })
        }
    }

    let service = AppDeliveryService::new(
        source_identity(),
        Activating {
            inner: activator,
            transport: transport.clone(),
            delay: Duration::from_millis(5),
        },
        (*transport).clone(),
        DeliveryDraftStore::memory(),
    );

    let receipt = service
        .request_app(
            target,
            capability(),
            json!({"title": "wake and deliver"}),
            AppDeliveryOptions {
                request_id: Some("req-offline-1".into()),
                ready_timeout: Duration::from_secs(2),
                ..AppDeliveryOptions::default()
            },
        )
        .await
        .unwrap();

    assert!(matches!(receipt, DeliveryReceipt::Accepted { .. }));
    assert_eq!(
        service.phase_for("req-offline-1"),
        Some(DeliveryPhase::Accepted)
    );
}

#[tokio::test]
async fn process_started_but_not_ready_does_not_transmit_early() {
    let target = AppId::new("lilia.code").unwrap();
    let transport = InMemoryAppLinkTransport::new();
    transport.register_offline(&target, vec![capability()], Duration::from_secs(30));
    transport.mark_online(&target);

    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        transport,
        DeliveryDraftStore::memory(),
    );
    let error = service
        .request_app(
            target,
            capability(),
            json!({}),
            AppDeliveryOptions {
                request_id: Some("req-not-ready".into()),
                activate_if_offline: false,
                ready_timeout: Duration::from_millis(40),
                persist_on_failure: true,
                ..AppDeliveryOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error, AppDeliveryError::ReadyTimeout);
    assert!(service.drafts().get("req-not-ready").is_some());
    assert_eq!(
        service.phase_for("req-not-ready"),
        Some(DeliveryPhase::DeliveryFailed)
    );
}

#[tokio::test]
async fn duplicate_request_id_returns_previous_receipt() {
    let target = AppId::new("lilia.code").unwrap();
    let transport = InMemoryAppLinkTransport::new();
    transport.register_online(&target, vec![capability()]);
    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        transport,
        DeliveryDraftStore::memory(),
    );
    let options = options_with_id("req-dup");
    let first = service
        .request_app(
            target.clone(),
            capability(),
            json!({"n": 1}),
            options.clone(),
        )
        .await
        .unwrap();
    let second = service
        .request_app(target, capability(), json!({"n": 2}), options)
        .await
        .unwrap();
    assert!(matches!(first, DeliveryReceipt::Accepted { .. }));
    match second {
        DeliveryReceipt::Duplicate { previous, .. } => {
            assert!(matches!(*previous, DeliveryReceipt::Accepted { .. }));
        }
        other => panic!("expected duplicate, got {other:?}"),
    }
}

#[tokio::test]
async fn structured_errors_are_distinguishable() {
    let target = AppId::new("lilia.code").unwrap();
    let cases = [
        AppDeliveryError::ProtocolIncompatible,
        AppDeliveryError::CapabilityUnavailable,
        AppDeliveryError::PermissionDenied,
        AppDeliveryError::ActivationFailed {
            message: "spawn failed".into(),
        },
    ];
    for expected in cases {
        let transport = InMemoryAppLinkTransport::new();
        transport.register_online(&target, vec![capability()]);
        transport.set_force_error(&target, expected.clone());
        let service = AppDeliveryService::new(
            source_identity(),
            ProcessAppActivator::new(),
            transport,
            DeliveryDraftStore::memory(),
        );
        let error = service
            .request_app(
                target.clone(),
                capability(),
                json!({}),
                AppDeliveryOptions {
                    persist_on_failure: false,
                    ..AppDeliveryOptions::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind_name(), expected.kind_name());
    }

    let missing = AppId::new("missing.app").unwrap();
    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        InMemoryAppLinkTransport::new(),
        DeliveryDraftStore::memory(),
    );
    let error = service
        .request_app(
            missing,
            capability(),
            json!({}),
            AppDeliveryOptions {
                activate_if_offline: false,
                persist_on_failure: false,
                ..AppDeliveryOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error, AppDeliveryError::AppNotInstalled);
}

#[tokio::test]
async fn draft_saved_on_structured_failure() {
    let target = AppId::new("lilia.code").unwrap();
    let transport = InMemoryAppLinkTransport::new();
    transport.register_online(&target, vec![capability()]);
    transport.set_force_error(
        &target,
        AppDeliveryError::DeliveryFailed {
            message: "boom".into(),
        },
    );
    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        transport,
        DeliveryDraftStore::memory(),
    );
    let request_id = "req-draft";
    let _ = service
        .request_app(
            target,
            capability(),
            json!({"keep": true}),
            options_with_id(request_id),
        )
        .await
        .unwrap_err();
    let draft = service.drafts().get(request_id).unwrap();
    assert_eq!(draft.capability.name, "lilia.code.task.accept");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_link_roundtrip_delivers_typed_receipt() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let lease_dir = std::env::temp_dir().join(format!("mutsuki-delivery-{unique}"));
    let target = AppId::new(format!("demo.app{unique}")).unwrap();
    let endpoint = AppCapabilityEndpoint::open(target.clone(), "code-1", &lease_dir).unwrap();
    endpoint.register_handler(capability(), |envelope| DeliveryReceipt::Accepted {
        request_id: envelope.request_id,
        remote_task_id: Some("task-local-1".into()),
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let transport =
        LinkLocalAppTransport::new(&lease_dir).with_request_timeout(Duration::from_secs(5));
    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        transport,
        DeliveryDraftStore::memory(),
    );
    let receipt = service
        .request_app(
            target,
            capability(),
            json!({"title": "local ipc"}),
            AppDeliveryOptions {
                request_id: Some("req-local-1".into()),
                activate_if_offline: false,
                ready_timeout: Duration::from_secs(2),
                request_timeout: Duration::from_secs(5),
                persist_on_failure: false,
            },
        )
        .await
        .expect("local transmit");
    assert!(matches!(
        receipt,
        DeliveryReceipt::Accepted {
            remote_task_id: Some(ref id),
            ..
        } if id == "task-local-1"
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_link_waits_for_delayed_handler_registration() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let lease_dir = std::env::temp_dir().join(format!("mutsuki-delivery-delay-{unique}"));
    let target = AppId::new(format!("demo.delay{unique}")).unwrap();
    let endpoint = AppCapabilityEndpoint::open(target.clone(), "code-delay", &lease_dir).unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;

    let transport = LinkLocalAppTransport::new(&lease_dir);
    let session = transport
        .try_connect(&target)
        .await
        .expect("endpoint listening");
    assert!(
        session.capabilities.is_empty(),
        "handler not registered yet must not report capabilities"
    );
    assert!(!session.capability_ready(&capability()));

    let endpoint_for_handler = endpoint.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        endpoint_for_handler.register_handler(capability(), |envelope| DeliveryReceipt::Accepted {
            request_id: envelope.request_id,
            remote_task_id: Some("task-delayed".into()),
        });
    });

    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        transport,
        DeliveryDraftStore::memory(),
    );
    let receipt = service
        .request_app(
            target,
            capability(),
            json!({"title": "wait for handler"}),
            AppDeliveryOptions {
                request_id: Some("req-delayed-handler".into()),
                activate_if_offline: false,
                ready_timeout: Duration::from_secs(2),
                request_timeout: Duration::from_secs(5),
                persist_on_failure: false,
            },
        )
        .await
        .expect("delivery must wait for delayed handler");
    assert!(matches!(
        receipt,
        DeliveryReceipt::Accepted {
            remote_task_id: Some(ref id),
            ..
        } if id == "task-delayed"
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_link_unregistered_capability_is_not_ready_before_timeout() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let lease_dir = std::env::temp_dir().join(format!("mutsuki-delivery-unreg-{unique}"));
    let target = AppId::new(format!("demo.unreg{unique}")).unwrap();
    let _endpoint = AppCapabilityEndpoint::open(target.clone(), "code-unreg", &lease_dir).unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;

    let transport = LinkLocalAppTransport::new(&lease_dir);
    let session = transport.try_connect(&target).await.expect("listening");
    assert!(session.capabilities.is_empty());

    let started = tokio::time::Instant::now();
    let error = transport
        .wait_capability_ready(&session, &capability(), Duration::from_millis(80))
        .await
        .expect_err("must not synthesize Ready");
    assert_eq!(error, AppDeliveryError::ReadyTimeout);
    assert!(started.elapsed() >= Duration::from_millis(70));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_link_legacy_peer_is_protocol_incompatible() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let lease_dir = std::env::temp_dir().join(format!("mutsuki-delivery-legacy-{unique}"));
    let target = AppId::new(format!("demo.legacy{unique}")).unwrap();
    spawn_legacy_endpoint(target.clone(), lease_dir.clone()).await;

    let transport =
        LinkLocalAppTransport::new(&lease_dir).with_request_timeout(Duration::from_secs(2));
    let error = transport
        .try_connect(&target)
        .await
        .expect_err("legacy peer must fail handshake");
    assert_eq!(error, AppDeliveryError::ProtocolIncompatible);

    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        transport,
        DeliveryDraftStore::memory(),
    );
    let error = service
        .request_app(
            target,
            capability(),
            json!({"title": "legacy"}),
            AppDeliveryOptions {
                request_id: Some("req-legacy".into()),
                activate_if_offline: false,
                ready_timeout: Duration::from_millis(200),
                request_timeout: Duration::from_secs(2),
                persist_on_failure: true,
            },
        )
        .await
        .expect_err("must fail before payload transmit");
    assert_eq!(error, AppDeliveryError::ProtocolIncompatible);
    assert!(service.drafts().get("req-legacy").is_some());
}

#[tokio::test]
async fn protocol_version_mismatch_fails_before_transmit() {
    let target = AppId::new("lilia.code").unwrap();
    let transport = InMemoryAppLinkTransport::new();
    transport.register_online(&target, vec![capability()]);
    transport.set_host_protocol_version(&target, HOST_PROTOCOL_VERSION + 1);
    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        transport,
        DeliveryDraftStore::memory(),
    );
    let error = service
        .request_app(
            target,
            capability(),
            json!({}),
            AppDeliveryOptions {
                request_id: Some("req-proto".into()),
                activate_if_offline: false,
                persist_on_failure: true,
                ..AppDeliveryOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error, AppDeliveryError::ProtocolIncompatible);
    assert!(service.drafts().get("req-proto").is_some());
}

#[tokio::test]
async fn incompatible_capability_schema_returns_protocol_incompatible() {
    let target = AppId::new("lilia.code").unwrap();
    let transport = InMemoryAppLinkTransport::new();
    transport.register_online(
        &target,
        vec![CapabilityDescriptor::new("lilia.code.task.accept", 1, 99)],
    );
    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        transport,
        DeliveryDraftStore::memory(),
    );
    let error = service
        .request_app(
            target,
            capability(),
            json!({}),
            AppDeliveryOptions {
                request_id: Some("req-schema".into()),
                activate_if_offline: false,
                persist_on_failure: true,
                ..AppDeliveryOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error, AppDeliveryError::ProtocolIncompatible);
    assert!(service.drafts().get("req-schema").is_some());
}

#[tokio::test]
async fn missing_capability_after_ready_wait_is_capability_unavailable() {
    let target = AppId::new("lilia.code").unwrap();
    let transport = InMemoryAppLinkTransport::new();
    transport.register_online(&target, Vec::new());
    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        transport.clone(),
        DeliveryDraftStore::memory(),
    );

    // Simulate a peer that never advertises the required capability.
    let error = service
        .request_app(
            target.clone(),
            capability(),
            json!({}),
            AppDeliveryOptions {
                request_id: Some("req-missing-cap".into()),
                activate_if_offline: false,
                ready_timeout: Duration::from_millis(30),
                persist_on_failure: true,
                ..AppDeliveryOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error, AppDeliveryError::ReadyTimeout);
    assert!(service.drafts().get("req-missing-cap").is_some());

    // If handshake later reports a different capability only, transmit must still fail structured.
    transport.set_capabilities(
        &target,
        vec![CapabilityDescriptor::new("other.capability", 1, 1)],
    );
    let session = transport.try_connect(&target).await.unwrap();
    let error = transport
        .transmit(
            &session,
            &mutsuki_runtime_contracts::CapabilityRequestEnvelope::new(
                "req-tx",
                "lilia.github",
                target.as_str(),
                capability(),
                json!({}),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(error, AppDeliveryError::CapabilityUnavailable);
}

async fn spawn_legacy_endpoint(target: AppId, lease_dir: std::path::PathBuf) {
    use mutsuki_link_core::{Connection, EndpointId, TransportBudget};
    use mutsuki_link_local::{
        EndpointLease, LocalListener, SessionIdentity, endpoint_id_for_app, local_address_for_app,
    };
    use mutsuki_runtime_contracts::CapabilityRequestEnvelope;

    let session = SessionIdentity::current();
    let link_app = mutsuki_link_local::AppId::new(target.as_str()).unwrap();
    let address = local_address_for_app(&link_app, &session);
    let endpoint_id = endpoint_id_for_app(&link_app, &session);
    let _lease = EndpointLease::create(&lease_dir, &link_app, "legacy-1").unwrap();
    let budget = TransportBudget {
        idle_timeout: None,
        ..TransportBudget::default()
    };
    let listener = LocalListener::bind(&address, endpoint_id, budget).unwrap();
    tokio::spawn(async move {
        let _lease = _lease;
        loop {
            let Ok(mut connection) = listener.accept(EndpointId::from_bytes([1; 16])).await else {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            };
            // Old peers only accepted bare CapabilityRequestEnvelope frames.
            let _ = connection.try_receive();
            let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
            loop {
                match connection.try_receive() {
                    Ok(Some(bytes)) => {
                        let _ = serde_json::from_slice::<CapabilityRequestEnvelope>(&bytes);
                        break;
                    }
                    Ok(None) => break,
                    Err(_) if tokio::time::Instant::now() >= deadline => break,
                    Err(_) => tokio::task::yield_now().await,
                }
            }
            let _ = connection.close_write();
        }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
}
