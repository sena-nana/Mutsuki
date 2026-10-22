//! Runner, manifest and polling behavior tests.

use super::*;

#[test]
fn management_manifest_exposes_a_behavior_node_without_a_command_name() {
    let base = manifest();
    assert!(
        base.requires
            .contains(&SurfaceRequirement::task_protocol(CARD_RENDER))
    );
    // The runner accepts the management command and requires the QR
    // renderer regardless of the management switch.
    assert!(
        base.requires
            .contains(&SurfaceRequirement::task_protocol(QR_RENDER))
    );
    assert!(
        base.provides.runners[0]
            .accepted_protocol_ids
            .iter()
            .any(|protocol_id| protocol_id == MANAGEMENT_COMMAND)
    );
    let base_nodes =
        BotNodeCatalogFragment::from_plugin_extension(base.provides.extensions.first().unwrap())
            .unwrap()
            .unwrap();
    #[allow(clippy::items_after_statements)]
    fn node_ids(nodes: &BotNodeCatalogFragment) -> Vec<&str> {
        nodes
            .nodes
            .iter()
            .map(|node| node.node_type_id.as_str())
            .collect()
    }
    assert_eq!(
        node_ids(&base_nodes),
        [
            BILIBILI_NOTIFICATION_NODE_TYPE,
            BILIBILI_CARD_NODE_TYPE,
            "mutsuki.bot.bilibili.resolve"
        ]
    );
    let notification = &base_nodes.nodes[0];
    assert_eq!(notification.role, BotNodeRole::Source);
    assert!(notification.binding.is_none());
    let notification_port = &notification.ports[0];
    assert_eq!(notification_port.direction, BotNodePortDirection::Output);
    assert_eq!(notification_port.event_type.type_id, BILIBILI_EVENT_TYPE);
    assert_eq!(notification_port.event_type.version, 1);
    let card = &base_nodes.nodes[1];
    assert_eq!(card.role, BotNodeRole::Processor);
    assert_eq!(card.binding.as_ref().unwrap().protocol_id, NOTIFY_CARD);
    assert_eq!(card.ports[0].direction, BotNodePortDirection::Input);
    assert_eq!(card.ports[0].event_type.type_id, BILIBILI_EVENT_TYPE);
    assert_eq!(card.ports[1].event_type.type_id, "mutsuki.bot.message.send");

    let mut config = managed_config();
    let managed = manifest_for_config(&config);
    assert!(
        managed
            .requires
            .contains(&SurfaceRequirement::task_protocol(QR_RENDER))
    );
    assert!(
        managed.provides.runners[0]
            .accepted_protocol_ids
            .iter()
            .any(|protocol_id| protocol_id == MANAGEMENT_COMMAND)
    );
    let extension = managed.provides.extensions.first().unwrap();
    let nodes = BotNodeCatalogFragment::from_plugin_extension(extension)
        .unwrap()
        .unwrap();
    assert_eq!(
        node_ids(&nodes),
        [
            BILIBILI_NOTIFICATION_NODE_TYPE,
            BILIBILI_CARD_NODE_TYPE,
            "mutsuki.bot.bilibili.resolve",
            "mutsuki.bot.bilibili.management"
        ]
    );
    assert_eq!(
        nodes.nodes[3].binding.as_ref().unwrap().protocol_id,
        MANAGEMENT_COMMAND
    );
    assert!(
        managed.provides.runners[0]
            .accepted_protocol_ids
            .iter()
            .all(
                |protocol_id| managed.provides.protocol_classes.get(protocol_id.as_str())
                    == Some(&ProtocolClass::Effect)
            )
    );

    config.risk_control = Some(BilibiliRiskControlConfig {
        backend: BilibiliRiskControlBackend::Chromium,
        timeout_ms: 1_000,
        max_response_bytes: 1024,
    });
    let risk_control = manifest_for_config(&config);
    assert!(
        risk_control
            .requires
            .contains(&SurfaceRequirement::task_protocol(SNAPSHOT))
    );
    assert!(
        risk_control
            .requires
            .contains(&SurfaceRequirement::task_protocol(
                mutsuki_protocol_http::REQUEST
            ))
    );
    // Upstream I/O runs in the HTTP effect; the runner only awaits it.
    assert_eq!(
        risk_control.provides.runners[0].execution_class,
        ExecutionClass::Orchestration
    );
}

#[test]
fn explicit_chromium_backend_awaits_snapshot_and_reports_degraded_success() {
    let html = r#"<article class="bili-dyn-list__item">
            <a href="https://t.bilibili.com/42">detail</a>
            <div class="bili-rich-text">fallback item</div>
        </article>"#;
    let bytes = serde_json::to_vec(&BrowserSnapshot {
        final_url: "https://space.bilibili.com/7/dynamic".into(),
        title: "space".into(),
        html: html.into(),
    })
    .unwrap();
    let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
    let runner = BilibiliRunner::new(
        Arc::new(RiskControlledTransport),
        repository.clone(),
        Arc::new(SnapshotResources { bytes }),
        "memory",
    );
    let mut runner = runner.into_runtime_runner(
        Arc::new(CompletedChildClient),
        Some(BilibiliRiskControlConfig {
            backend: BilibiliRiskControlBackend::Chromium,
            timeout_ms: 5_000,
            max_response_bytes: 64 * 1024,
        }),
    );
    let task = Task::new(
        "risk-control",
        POLL_DYNAMIC,
        serde_json::to_value(PollRequest {
            subscription_id: "sub".into(),
            uid: 7,
            target: BotTarget::Group {
                group_id: "group".into(),
            },
            outbound_binding: "qq-main".into(),
        })
        .unwrap(),
    );
    let batch = command_batch(vec![task]);
    let context =
        RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
    let waiting = runner.run_batch(context.clone(), batch.clone()).unwrap();
    let waiting = waiting.results[0].result.as_ref().unwrap();
    assert_eq!(waiting.tasks[0].protocol_id, SNAPSHOT);
    assert!(waiting.task_await.is_some());

    let completed = runner.run_batch(context, batch).unwrap();
    let completed = completed.results[0].result.as_ref().unwrap();
    assert!(
        completed
            .events
            .iter()
            .any(|event| event.kind == RISK_CONTROL_STATUS_EVENT
                && event.payload["fallback"] == "succeeded")
    );
    assert_eq!(
        repository.cursor("Dynamic:7:sub").unwrap().as_deref(),
        Some("42")
    );
}

#[test]
fn link_resolution_awaits_card_renderer_then_sends_image_and_url() {
    let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
    let mut runner = BilibiliRunner::new(
        Arc::new(LinkTransport),
        repository.clone(),
        Arc::new(UnusedResources),
        "memory",
    )
    .into_runtime_runner(Arc::new(RenderedChildClient), None);
    let task = Task::new(
        "link-card",
        LINK_RESOLVE,
        serde_json::to_value(LinkResolveRequest {
            url: "https://www.bilibili.com/video/BV1Visual".into(),
            target: BotTarget::Group {
                group_id: "group".into(),
            },
            outbound_binding: "qq-main".into(),
            account_id: "account".into(),
            now_ms: 1_000,
            cooldown_ms: 60_000,
        })
        .unwrap(),
    );
    let batch = command_batch(vec![task]);
    let context =
        RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
    let waiting = runner.run_batch(context.clone(), batch.clone()).unwrap();
    let waiting = waiting.results[0].result.as_ref().unwrap();
    assert_eq!(waiting.tasks[0].protocol_id, CARD_RENDER);
    let request: CardRenderRequest =
        serde_json::from_value(waiting.tasks[0].payload.to_value()).unwrap();
    assert_eq!(request.brand, "哔哩哔哩");
    assert_eq!(request.layout, CardLayout::Media);
    assert_eq!(request.kicker, "投稿");
    assert_eq!(
        request.fallback_gradient.start,
        Rgba {
            red: 251,
            green: 114,
            blue: 153,
            alpha: 255,
        }
    );
    assert!(waiting.task_await.is_some());
    assert!(
        repository
            .cooldown_ready(
                "account:https://www.bilibili.com/video/BV1Visual",
                1_000,
                60_000,
            )
            .unwrap()
    );

    let completed = runner.run_batch(context, batch).unwrap();
    let completed = completed.results[0].result.as_ref().unwrap();
    assert_eq!(completed.tasks.len(), 1);
    let message: BotMessage =
        serde_json::from_value(completed.tasks[0].payload.to_value()).unwrap();
    assert!(matches!(message.segments[0], MessageSegment::Image { .. }));
    assert_eq!(
        message.segments[1],
        MessageSegment::Text {
            text: "https://www.bilibili.com/video/BV1Visual".into(),
        }
    );
    assert!(
        !repository
            .cooldown_ready(
                "account:https://www.bilibili.com/video/BV1Visual",
                1_000,
                60_000,
            )
            .unwrap()
    );
}

#[test]
fn link_resolution_from_flow_invocation_reads_event_url() {
    let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
    let mut runner = BilibiliRunner::new(
        Arc::new(LinkTransport),
        repository,
        Arc::new(UnusedResources),
        "memory",
    )
    .into_runtime_runner(Arc::new(RenderedChildClient), None);
    let event = BotEvent {
        event_id: "e1".into(),
        platform: BotPlatform::QqBot,
        bot: BotAccountRef {
            account_id: "account".into(),
            platform: BotPlatform::QqBot,
        },
        kind: BotEventKind::MessageCreated,
        time_ms: 1_000,
        target: BotTarget::Group {
            group_id: "group".into(),
        },
        actor: None,
        message: Some(BotMessage::text(
            BotTarget::Group {
                group_id: "group".into(),
            },
            "https://www.bilibili.com/video/BV1Visual",
        )),
        raw: None,
        ext: Default::default(),
    };
    let invocation = BotNodeInvocation {
        flow_id: "link".into(),
        graph_revision: 1,
        execution_id: "exec".into(),
        node_id: "bili".into(),
        input_port_id: "event".into(),
        wiring: BotNodeWiring::default(),
        config: json!({"outbound_binding": "qq-main", "cooldown_ms": 60_000}),
        input: BotFlowEventEnvelope {
            event_id: "e1".into(),
            protocol_id: mutsuki_bot_protocol::BOT_EVENT_INGEST_PROTOCOL_ID.into(),
            payload: BotFlowPayload {
                event_type: BotFlowTypeRef::new("mutsuki.bot.event", 1),
                value: serde_json::to_value(&event).unwrap(),
            },
            context: mutsuki_bot_protocol::BotFlowContext {
                bot: Some(event.bot.clone()),
                target: Some(event.target.clone()),
                actor: None,
                ext: Default::default(),
            },
            trace_id: None,
            correlation_id: None,
        },
    };
    let task = Task::new(
        "link-flow",
        LINK_RESOLVE,
        serde_json::to_value(invocation).unwrap(),
    );
    let batch = command_batch(vec![task]);
    let context =
        RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);
    let waiting = runner.run_batch(context.clone(), batch.clone()).unwrap();
    assert_eq!(
        waiting.results[0].result.as_ref().unwrap().tasks[0].protocol_id,
        CARD_RENDER
    );
    let completed = runner.run_batch(context, batch).unwrap();
    let completed = completed.results[0].result.as_ref().unwrap();
    assert!(completed.tasks.is_empty());
    let node: BotNodeResult = serde_json::from_value(completed.output.clone().unwrap()).unwrap();
    assert_eq!(node.outputs[0].port_id, "message");
    let message: BotMessage =
        serde_json::from_value(node.outputs[0].event.payload.value.clone()).unwrap();
    assert!(matches!(message.segments[0], MessageSegment::Image { .. }));
}

#[test]
fn multiple_fresh_items_are_emitted_as_flow_events_in_order() {
    let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
    repository.set_cursor("Dynamic:7:sub", "1").unwrap();
    let mut runner = BilibiliRunner::new(
        Arc::new(MultiPollTransport),
        repository.clone(),
        Arc::new(UnusedResources),
        "memory",
    )
    .into_runtime_runner(Arc::new(CompletedChildClient), None);
    let task = Task::new(
        "multi-notify",
        POLL_DYNAMIC,
        serde_json::to_value(PollRequest {
            subscription_id: "sub".into(),
            uid: 7,
            target: BotTarget::Group {
                group_id: "group".into(),
            },
            outbound_binding: "qq-main".into(),
        })
        .unwrap(),
    );
    let batch = command_batch(vec![task]);
    let context =
        RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);

    let completed = runner.run_batch(context, batch).unwrap();
    let completed = completed.results[0].result.as_ref().unwrap();
    assert_eq!(completed.tasks.len(), 2);
    assert!(
        completed
            .tasks
            .iter()
            .all(|task| task.protocol_id == BOT_FLOW_INGRESS_PROTOCOL_ID)
    );
    let urls = completed
        .tasks
        .iter()
        .map(|task| {
            let envelope: BotFlowEventEnvelope =
                serde_json::from_value(task.payload.to_value()).unwrap();
            assert_eq!(
                envelope.protocol_id,
                mutsuki_bot_protocol::BOT_EVENT_INGEST_PROTOCOL_ID
            );
            assert_eq!(envelope.payload.event_type.type_id, BILIBILI_EVENT_TYPE);
            let notification: BilibiliNotification =
                serde_json::from_value(envelope.payload.value).unwrap();
            assert_eq!(notification.kind, BilibiliPollKind::Dynamic);
            assert_eq!(notification.uid, 7);
            assert_eq!(
                notification.target,
                BotTarget::Group {
                    group_id: "group".into(),
                }
            );
            notification.url
        })
        .collect::<Vec<_>>();
    assert_eq!(
        urls,
        vec![
            "https://t.bilibili.com/2".to_string(),
            "https://t.bilibili.com/3".to_string(),
        ]
    );
    assert_eq!(repository.cursor("Dynamic:7:sub").unwrap().unwrap(), "3");
}

#[test]
fn notification_card_node_renders_subscription_payload_into_message_send() {
    let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
    let mut runner = BilibiliRunner::new(
        Arc::new(LinkTransport),
        repository,
        Arc::new(UnusedResources),
        "memory",
    )
    .into_runtime_runner(Arc::new(RenderedChildClient), None);
    let notification = BilibiliNotification {
        kind: BilibiliPollKind::Live,
        subscription_id: "sub".into(),
        uid: 42,
        target: BotTarget::Group {
            group_id: "group".into(),
        },
        item_id: "true".into(),
        title: "晚上 eight 点开播".into(),
        url: "https://live.bilibili.com/1000".into(),
        image_url: None,
    };
    let invocation = BotNodeInvocation {
        flow_id: "push".into(),
        graph_revision: 1,
        execution_id: "exec".into(),
        node_id: "card".into(),
        input_port_id: "event".into(),
        wiring: BotNodeWiring::default(),
        config: json!({}),
        input: BotFlowEventEnvelope {
            event_id: "notify-1".into(),
            protocol_id: mutsuki_bot_protocol::BOT_EVENT_INGEST_PROTOCOL_ID.into(),
            payload: BotFlowPayload {
                event_type: BotFlowTypeRef::new(BILIBILI_EVENT_TYPE, 1),
                value: serde_json::to_value(&notification).unwrap(),
            },
            context: mutsuki_bot_protocol::BotFlowContext {
                bot: None,
                target: Some(notification.target.clone()),
                actor: None,
                ext: BotExtMap::new(),
            },
            trace_id: None,
            correlation_id: None,
        },
    };
    let task = Task::new(
        "notify-card",
        NOTIFY_CARD,
        serde_json::to_value(invocation).unwrap(),
    );
    let batch = command_batch(vec![task]);
    let context =
        RunnerContext::new(1, 1, "executor", None::<&str>, "invocation").with_batch("batch", 1);

    let waiting = runner.run_batch(context.clone(), batch.clone()).unwrap();
    let waiting = waiting.results[0].result.as_ref().unwrap();
    assert_eq!(waiting.tasks[0].protocol_id, CARD_RENDER);
    let request: CardRenderRequest =
        serde_json::from_value(waiting.tasks[0].payload.to_value()).unwrap();
    assert_eq!(request.layout, CardLayout::Row);
    assert_eq!(request.kicker, "直播");
    assert!(request.live);
    assert_eq!(request.cover, None);
    assert!(waiting.task_await.is_some());

    let completed = runner.run_batch(context, batch).unwrap();
    let completed = completed.results[0].result.as_ref().unwrap();
    let output = completed.output.as_ref().unwrap();
    let node_result: BotNodeResult = serde::Deserialize::deserialize(output).unwrap();
    assert_eq!(node_result.outputs.len(), 1);
    let output = &node_result.outputs[0];
    assert_eq!(output.port_id, "message");
    assert_eq!(
        output.event.payload.event_type.type_id,
        "mutsuki.bot.message.send"
    );
    let message: BotMessage = serde::Deserialize::deserialize(&output.event.payload.value).unwrap();
    assert_eq!(
        message.target,
        BotTarget::Group {
            group_id: "group".into(),
        }
    );
    assert!(matches!(message.segments[0], MessageSegment::Image { .. }));
    assert_eq!(
        message.segments[1],
        MessageSegment::Text {
            text: "https://live.bilibili.com/1000".into(),
        }
    );
}

#[test]
fn risk_control_config_rejects_unbounded_limits() {
    let mut config = managed_config();
    config.risk_control = Some(BilibiliRiskControlConfig {
        backend: BilibiliRiskControlBackend::Chromium,
        timeout_ms: 0,
        max_response_bytes: 1024,
    });
    assert!(config.validate().unwrap_err().contains("timeout_ms"));
    config.risk_control.as_mut().unwrap().timeout_ms = 1000;
    config.risk_control.as_mut().unwrap().max_response_bytes = 0;
    assert!(
        config
            .validate()
            .unwrap_err()
            .contains("max_response_bytes")
    );
}

#[test]
fn unwired_push_chain_skips_polling_and_reports_the_freeze() {
    let polls = Arc::new(AtomicU64::new(0));
    let mut runner = count_runner(polls.clone(), Some(push_registry(false)));

    let result = run_poll_task(&mut runner, "frozen-poll");
    assert_eq!(
        polls.load(Ordering::SeqCst),
        0,
        "unwired chain must not poll"
    );
    assert_eq!(result.output.as_ref().unwrap()["poll_skipped"], json!(true));
    assert_eq!(result.output.as_ref().unwrap()["push_wired"], json!(false));
}

#[test]
fn wired_push_chain_polls_and_fans_out_notifications() {
    let polls = Arc::new(AtomicU64::new(0));
    let mut runner = count_runner(polls.clone(), Some(push_registry(true)));

    let first = run_poll_task(&mut runner, "first-poll");
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    assert!(first.tasks.is_empty(), "first poll baselines the cursor");

    let second = run_poll_task(&mut runner, "second-poll");
    assert_eq!(polls.load(Ordering::SeqCst), 2);
    assert_eq!(second.tasks.len(), 1);
    assert_eq!(second.tasks[0].protocol_id, BOT_FLOW_INGRESS_PROTOCOL_ID);
}

#[test]
fn forced_baseline_poll_advances_the_cursor_without_notifications() {
    let repository = Arc::new(SqliteBilibiliRepository::open(":memory:").unwrap());
    repository.set_cursor("Live:7:sub", "item-8").unwrap();
    let mut runner = BilibiliRunner::new(
        Arc::new(CountingTransport {
            polls: Arc::new(AtomicU64::new(0)),
        }),
        repository.clone(),
        Arc::new(UnusedResources),
        "memory",
    );
    let request = PollRequest {
        subscription_id: "sub".into(),
        uid: 7,
        target: BotTarget::Group {
            group_id: "group".into(),
        },
        outbound_binding: "qq-main".into(),
    };
    let task = Task::new(
        "baseline",
        POLL_LIVE,
        serde_json::to_value(&request).unwrap(),
    );
    let items = vec![BilibiliItem {
        id: "item-9".into(),
        title: "newest".into(),
        url: "https://t.bilibili.com/9".into(),
        image_url: None,
    }];

    let result = runner
        .finish_poll(&task, request, BilibiliPollKind::Live, items, None, true)
        .unwrap();
    assert!(
        result.tasks.is_empty(),
        "the frozen window must not replay as a notification backlog"
    );
    assert_eq!(
        repository.cursor("Live:7:sub").unwrap().as_deref(),
        Some("item-9")
    );
}
