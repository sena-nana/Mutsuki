# Cross-app delivery (request_app)

`MutsukiTauriHost` owns waking a peer Tauri app and delivering a typed capability request over
MutsukiLink local IPC (Named Pipe / UDS).

```rust
use mutsuki_tauri_host::{
    AppDeliveryOptions, AppDeliveryService, AppDescriptor, AppId, AppIdentity,
    CapabilityDescriptor, DeliveryDraftStore, InMemoryAppLinkTransport, ProcessAppActivator,
};
use serde_json::json;

let source = AppIdentity {
    app_id: AppId::new("lilia.github")?,
    instance_id: "instance-1".into(),
};
let transport = InMemoryAppLinkTransport::new();
let target = AppId::new("lilia.code")?;
transport.register_online(
    &target,
    vec![CapabilityDescriptor::new("lilia.code.task.accept", 1, 1)],
);
let activator = ProcessAppActivator::new();
activator
    .register(AppDescriptor {
        app_id: target.clone(),
        display_name: "LiliaCode".into(),
        executable: None,
        launch_args: vec![],
        bundle_id: None,
    })
    .await;

let delivery = AppDeliveryService::new(
    source,
    activator,
    transport,
    DeliveryDraftStore::memory(),
);

let receipt = delivery
    .request_app(
        target,
        CapabilityDescriptor::new("lilia.code.task.accept", 1, 1),
        json!({ "title": "fix CI" }),
        AppDeliveryOptions::default(),
    )
    .await?;
```

Rules:

- Do not put the full business task into argv or treat a handoff file as the default online path.
- Persist `DeliveryDraft` only after structured delivery failure; drafts are recovery artifacts.
- Wait for capability ready, not merely process start, before transmit.
- Ordinary app-to-app traffic does not require `MutsukiServiceHost`.
