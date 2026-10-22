//! Chat command and management service tests.

use super::*;

#[test]
fn login_command_awaits_qr_renderer_then_sends_rendered_resource() {
    let state = Arc::new(Mutex::new(FakeTransportState::default()));
    let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
    let config = SharedBilibiliConfig::new(managed_config());
    let management = Arc::new(BilibiliManagementService::new(
        config.clone(),
        SharedBilibiliCredential::default(),
        Box::new(FakeTransport(state.clone())),
        repository.clone(),
        Arc::new(RecordingCredentialStore::default()),
        Arc::new(RecordingConfigStore::default()),
        Arc::new(AlwaysPresentSecrets),
    ));
    let mut runner = BilibiliRunner::new(
        Box::new(FakeTransport(state)),
        repository,
        Arc::new(UnusedResources),
        "memory",
    )
    .with_management(config, management)
    .into_runtime_runner(Arc::new(RenderedChildClient), None);
    let task = command_task("login-render", "admin", &["login"]);
    let batch = command_batch(vec![task]);
    let context =
        RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
    let waiting = runner.run_batch(context.clone(), batch.clone()).unwrap();
    let waiting = waiting.results[0].result.as_ref().unwrap();
    assert_eq!(waiting.tasks[0].protocol_id, QR_RENDER);
    let request: QrRenderRequest =
        serde_json::from_value(waiting.tasks[0].payload.to_value()).unwrap();
    assert_eq!(request.content, "https://passport.bilibili.com/qr");
    assert_eq!(request.min_dimensions, 256);

    let completed = runner.run_batch(context, batch).unwrap();
    let completed = completed.results[0].result.as_ref().unwrap();
    let message: BotMessage =
        serde_json::from_value(completed.tasks[0].payload.to_value()).unwrap();
    assert!(matches!(message.segments[0], MessageSegment::Image { .. }));
    assert!(matches!(message.segments[1], MessageSegment::Text { .. }));
}

#[test]
fn chat_login_is_forbidden_when_management_is_off_and_admin_list_is_empty() {
    let mut config = managed_config();
    config.management.enabled = false;
    config.management.admin_user_ids.clear();
    let (state, mut runner) = login_runtime_runner(config);
    let batch = command_batch(vec![command_task("login-empty", "admin", &["login"])]);
    let context =
        RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
    let completed = runner.run_batch(context, batch).unwrap();
    let error = completed.results[0]
        .error
        .as_ref()
        .expect("empty admin list must reject chat login");
    assert_eq!(error.code, "bilibili.management_forbidden");
    assert_eq!(state.lock().unwrap().qr_starts, 0);
}

#[test]
fn chat_login_start_is_allowed_for_admins_when_management_is_off() {
    let mut config = managed_config();
    config.management.enabled = false;
    let (state, mut runner) = login_runtime_runner(config);
    let batch = command_batch(vec![command_task("login-admin", "admin", &["login"])]);
    let context =
        RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
    let waiting = runner.run_batch(context, batch).unwrap();
    let waiting = waiting.results[0]
        .result
        .as_ref()
        .expect("chat admin login must reach QR render");
    assert_eq!(waiting.tasks[0].protocol_id, QR_RENDER);
    assert_eq!(state.lock().unwrap().qr_starts, 1);
}

#[test]
fn chat_login_status_requires_admin_even_when_management_is_off() {
    let mut config = managed_config();
    config.management.enabled = false;
    let shared = SharedBilibiliConfig::new(config);
    let state = Arc::new(Mutex::new(FakeTransportState::default()));
    let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
    let management = Arc::new(BilibiliManagementService::new(
        shared.clone(),
        SharedBilibiliCredential::default(),
        Box::new(FakeTransport(state)),
        repository.clone(),
        Arc::new(RecordingCredentialStore::default()),
        Arc::new(RecordingConfigStore::default()),
        Arc::new(AlwaysPresentSecrets),
    ));
    let runner = BilibiliRunner::new(
        Box::new(FakeTransport(Arc::new(Mutex::new(
            FakeTransportState::default(),
        )))),
        repository.clone(),
        Arc::new(UnusedResources),
        "memory",
    )
    .with_management(shared.clone(), management);
    repository.set_qr_session("admin", "qr-key").unwrap();
    runner
        .commands()
        .run_command(&command_task(
            "login-status-admin",
            "admin",
            &["login-status"],
        ))
        .unwrap();

    shared.replace({
        let mut next = shared.snapshot();
        next.management.admin_user_ids.clear();
        next
    });
    let forbidden = runner
        .commands()
        .run_command(&command_task(
            "login-status-empty",
            "admin",
            &["login-status"],
        ))
        .unwrap_err();
    assert_eq!(forbidden.code, "bilibili.management_forbidden");
}

#[test]
fn management_flow_rotates_secret_persists_verified_binding_and_previews_without_cursor() {
    let state = Arc::new(Mutex::new(FakeTransportState::default()));
    let config = SharedBilibiliConfig::new(managed_config());
    let credential = SharedBilibiliCredential::default();
    let credential_store = Arc::new(RecordingCredentialStore::default());
    let config_store = Arc::new(RecordingConfigStore::default());
    let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
    let management = Arc::new(BilibiliManagementService::new(
        config.clone(),
        credential,
        Box::new(FakeTransport(state.clone())),
        repository.clone(),
        credential_store.clone(),
        config_store.clone(),
        Arc::new(AlwaysPresentSecrets),
    ));
    let runner = BilibiliRunner::new(
        Box::new(FakeTransport(state.clone())),
        repository.clone(),
        Arc::new(UnusedResources),
        "memory",
    )
    .with_management(config.clone(), management.clone());

    repository.set_qr_session("admin", "qr-key").unwrap();
    let login = runner
        .commands()
        .run_command(&command_task("login", "admin", &["login-status"]))
        .unwrap();
    assert!(
        runner
            .commands()
            .run_command(&command_task("forbidden-login", "alice", &["login-status"]))
            .is_err()
    );
    assert_eq!(credential_store.0.lock().unwrap().len(), 1);
    assert!(
        !serde_json::to_string(&login.tasks)
            .unwrap()
            .contains("ROTATED")
    );

    runner
        .commands()
        .run_command(&command_task("bind", "alice", &["bind", "42"]))
        .unwrap();
    let (_, code) = repository.binding_challenge("alice").unwrap().unwrap();
    state.lock().unwrap().signature = format!("hello {code}");
    runner
        .commands()
        .run_command(&command_task("verify", "alice", &["verify"]))
        .unwrap();
    let snapshot = config.snapshot();
    assert_eq!(snapshot.subscriptions.len(), 1);
    assert_eq!(snapshot.subscriptions[0].uid, 42);
    assert_eq!(
        snapshot.subscriptions[0].owner_user_id.as_deref(),
        Some("alice")
    );

    runner
        .commands()
        .run_command(&command_task("pause", "alice", &["pause"]))
        .unwrap();
    assert!(config.snapshot().subscriptions[0].paused);
    assert!(config_store.0.lock().unwrap().len() >= 2);

    assert!(repository.cursor("Dynamic:42").unwrap().is_none());
    let preview = management.preview("alice", false, None).unwrap();
    assert_eq!(preview.title, "latest");
    assert!(repository.cursor("Dynamic:42").unwrap().is_none());
}

#[tokio::test]
async fn management_service_web_subscribe_and_clear_never_echo_cookie() {
    let config = SharedBilibiliConfig::new(managed_config());
    let credential = SharedBilibiliCredential::default();
    credential.set("SESSDATA=secret-cookie".into());
    let credential_store = Arc::new(RecordingCredentialStore::default());
    let config_store = Arc::new(RecordingConfigStore::default());
    let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
    let management = BilibiliManagementService::new(
        config.clone(),
        credential.clone(),
        Box::new(FakeTransport(Arc::new(Mutex::new(
            FakeTransportState::default(),
        )))),
        repository,
        credential_store.clone(),
        config_store.clone(),
        Arc::new(AlwaysPresentSecrets),
    );
    let status = management.status();
    let mut changes = management
        .subscribe_changes()
        .expect("Bilibili change source");
    assert!(status.credential_loaded);
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("secret-cookie")
    );

    let view = management
        .subscribe(
            "sub-1".into(),
            7,
            vec![BilibiliNotificationKind::Live],
            BotTarget::Group {
                group_id: "g1".into(),
            },
            "qq-main".into(),
        )
        .unwrap();
    assert_eq!(view.subscription_id, "sub-1");
    assert_eq!(config.snapshot().subscriptions.len(), 1);
    assert_eq!(config_store.0.lock().unwrap().len(), 1);
    let changed = changes.changed().await.expect("subscription change");
    assert!(
        changed
            .areas
            .contains(&mutsuki_bot_management::BilibiliManagementChangeArea::Subscriptions)
    );

    management.credential_clear().unwrap();
    assert!(!credential.is_loaded());
    assert_eq!(credential_store.0.lock().unwrap().last().unwrap().1, "");
}

#[test]
fn management_service_subscribe_rejects_duplicate_id_and_empty_target() {
    let mut seed = managed_config();
    seed.subscriptions.push(BilibiliSubscription {
        subscription_id: "sub-1".into(),
        uid: 42,
        notifications: vec![BilibiliPollKind::Dynamic],
        target: BotTarget::Group {
            group_id: "g1".into(),
        },
        outbound_binding: "qq-main".into(),
        paused: false,
        owner_user_id: Some("alice".into()),
    });
    let config = SharedBilibiliConfig::new(seed);
    let config_store = Arc::new(RecordingConfigStore::default());
    let management = BilibiliManagementService::new(
        config.clone(),
        SharedBilibiliCredential::default(),
        Box::new(FakeTransport(Arc::new(Mutex::new(
            FakeTransportState::default(),
        )))),
        Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap()),
        Arc::new(RecordingCredentialStore::default()),
        config_store.clone(),
        Arc::new(AlwaysPresentSecrets),
    );

    assert_eq!(
        management
            .subscribe(
                "sub-1".into(),
                7,
                vec![BilibiliNotificationKind::Live],
                BotTarget::Group {
                    group_id: "g2".into(),
                },
                "qq-main".into(),
            )
            .unwrap_err()
            .code,
        "bilibili.request_failed"
    );
    assert_eq!(
        config.snapshot().subscriptions[0].owner_user_id.as_deref(),
        Some("alice")
    );

    assert_eq!(
        management
            .subscribe(
                "sub-2".into(),
                7,
                vec![BilibiliNotificationKind::Live],
                BotTarget::Group {
                    group_id: "".into(),
                },
                "qq-main".into(),
            )
            .unwrap_err()
            .code,
        "bilibili.request_failed"
    );
    assert_eq!(config.snapshot().subscriptions.len(), 1);
    assert!(config_store.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn management_login_uses_bound_renderer_for_web_console_payload() {
    let management = BilibiliManagementService::new(
        SharedBilibiliConfig::new(managed_config()),
        SharedBilibiliCredential::default(),
        Box::new(FakeTransport(Arc::new(Mutex::new(
            FakeTransportState::default(),
        )))),
        Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap()),
        Arc::new(RecordingCredentialStore::default()),
        Arc::new(RecordingConfigStore::default()),
        Arc::new(AlwaysPresentSecrets),
    );
    management.bind_qr_renderer(Arc::new(FakeQrRenderer));
    let result = management.login_start("web-console").await.unwrap();
    assert_eq!(result.url, "https://passport.bilibili.com/qr");
    assert_eq!(result.qr_png, vec![1, 2, 3, 4]);
    assert_eq!(result.qr_png_base64, "AQIDBA==");
}

#[tokio::test]
async fn management_login_rotates_credentials_without_the_management_switch() {
    let mut config = managed_config();
    config.management.enabled = false;
    let credential = SharedBilibiliCredential::default();
    let credential_store = Arc::new(RecordingCredentialStore::default());
    let config_store = Arc::new(RecordingConfigStore::default());
    let management = BilibiliManagementService::new(
        SharedBilibiliConfig::new(config),
        credential.clone(),
        Box::new(FakeTransport(Arc::new(Mutex::new(
            FakeTransportState::default(),
        )))),
        Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap()),
        credential_store.clone(),
        config_store,
        Arc::new(AlwaysPresentSecrets),
    );
    management.bind_qr_renderer(Arc::new(FakeQrRenderer));
    management.login_start("web-console").await.unwrap();
    let polled = management.login_poll("web-console").unwrap();
    assert_eq!(polled.status, BilibiliQrLoginStatus::Confirmed);
    assert!(credential.is_loaded());
    assert_eq!(
        credential_store.0.lock().unwrap().last().unwrap(),
        &("BILIBILI_COOKIE".into(), "SESSDATA=ROTATED".into())
    );
    for code in [
        management
            .subscribe(
                "sub-1".into(),
                7,
                vec![BilibiliNotificationKind::Live],
                BotTarget::Group {
                    group_id: "g1".into(),
                },
                "qq-main".into(),
            )
            .unwrap_err()
            .code,
        management.list("admin", true).unwrap_err().code,
        management
            .set_paused("admin", true, None, true)
            .unwrap_err()
            .code,
        management.preview("admin", true, None).unwrap_err().code,
        management.unbind("admin").unwrap_err().code,
    ] {
        assert_eq!(code, "bilibili.management_unavailable");
    }
    assert_eq!(
        management.bind_start("admin", 7, "seed").unwrap_err().code,
        "bilibili.management_forbidden"
    );
}

#[test]
fn management_status_reports_push_wiring_only_with_a_shared_registry() {
    let status =
        managed_status_service(Arc::new(Mutex::new(FakeTransportState::default()))).status();
    assert_eq!(status.push_wired, None);

    let status = managed_status_service(Arc::new(Mutex::new(FakeTransportState::default())))
        .with_flow_registry(push_registry(false))
        .status();
    assert_eq!(status.push_wired, Some(false));

    let status = managed_status_service(Arc::new(Mutex::new(FakeTransportState::default())))
        .with_flow_registry(push_registry(true))
        .status();
    assert_eq!(status.push_wired, Some(true));
}
