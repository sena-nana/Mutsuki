use super::activator::{ProcessAppActivator, TauriAppActivator};
use super::delivery::AppDeliveryService;
use super::draft::DeliveryDraftStore;
use super::endpoint::{AppCapabilityEndpoint, connect_and_transmit};
use super::transport::InMemoryAppLinkTransport;
use super::types::{
    AppDeliveryError, AppDeliveryOptions, AppDescriptor, AppId, AppIdentity, DeliveryPhase,
};
use mutsuki_link_local::SessionIdentity;
use mutsuki_runtime_contracts::{CapabilityDescriptor, CapabilityRequestEnvelope, DeliveryReceipt};
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

#[tokio::test]
async fn online_direct_delivery_returns_accepted_receipt() {
    let target = AppId::new("lilia.code").unwrap();
    let transport = InMemoryAppLinkTransport::new();
    transport.register_online(&target, vec![capability()]);
    let activator = ProcessAppActivator::new();
    let service = AppDeliveryService::new(
        source_identity(),
        activator,
        transport,
        DeliveryDraftStore::memory(),
    );

    let receipt = service
        .deliver_to_app(
            "req-online-1",
            target,
            capability(),
            json!({"title": "fix CI"}),
            AppDeliveryOptions::default(),
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
    transport.register_offline_with_activation(
        &target,
        vec![capability()],
        Duration::from_millis(5),
        Duration::from_millis(20),
    );

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
        .deliver_to_app(
            "req-offline-1",
            target,
            capability(),
            json!({"title": "wake and deliver"}),
            AppDeliveryOptions {
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
    transport.register_offline_with_activation(
        &target,
        vec![capability()],
        Duration::ZERO,
        Duration::from_secs(30),
    );
    transport.mark_online(&target);

    let service = AppDeliveryService::new(
        source_identity(),
        ProcessAppActivator::new(),
        transport,
        DeliveryDraftStore::memory(),
    );
    let error = service
        .deliver_to_app(
            "req-not-ready",
            target,
            capability(),
            json!({}),
            AppDeliveryOptions {
                activate_if_offline: false,
                ready_timeout: Duration::from_millis(40),
                persist_on_failure: true,
                ..AppDeliveryOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error, AppDeliveryError::ReadyTimeout);
    let draft = service.drafts().get("req-not-ready").unwrap();
    assert!(!draft.delivered);
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
    let first = service
        .deliver_to_app(
            "req-dup",
            target.clone(),
            capability(),
            json!({"n": 1}),
            AppDeliveryOptions::default(),
        )
        .await
        .unwrap();
    let second = service
        .deliver_to_app(
            "req-dup",
            target,
            capability(),
            json!({"n": 2}),
            AppDeliveryOptions::default(),
        )
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
async fn draft_saved_is_never_marked_delivered() {
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
        .deliver_to_app(
            request_id,
            target,
            capability(),
            json!({"keep": true}),
            AppDeliveryOptions::default(),
        )
        .await
        .unwrap_err();
    let draft = service.drafts().get(request_id).unwrap();
    assert!(!draft.delivered);
    assert_eq!(draft.capability_name, "lilia.code.task.accept");
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

    let envelope = CapabilityRequestEnvelope::new(
        "req-local-1",
        "lilia.github",
        target.as_str(),
        capability(),
        json!({"title": "local ipc"}),
    );
    let receipt = connect_and_transmit(
        &target,
        &SessionIdentity::current(),
        &envelope,
        Duration::from_secs(5),
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
