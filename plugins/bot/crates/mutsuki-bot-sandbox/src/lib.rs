//! Headless QQ conversation sandbox used by the Bot Web Console.

mod content;
mod service;
mod types;

pub use content::{
    SandboxContentRef, SandboxRefKind, hash_bytes, normalize_segments, parse_face_id,
    remap_sandbox_media_ids,
};
pub use service::{
    SandboxApi, SandboxChangeSubscription, SandboxHistoryStore, SandboxRuntime, SandboxService,
};
pub use types::*;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use mutsuki_bot_protocol::{
        BotAccountRef, BotConversationKind, BotEvent, BotEventKind, BotExtMap, BotMessage,
        BotPlatform, BotTarget, BotUser, MessageSegment, QQ_CONVERSATION_REF_VERSION,
        QqConversationRef,
    };
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceRef, ResourceSealState,
        ResourceSemantic,
    };
    use serde_json::{Value, json};

    use super::*;

    struct RecordingRuntime {
        ingest: std::sync::Mutex<Vec<BotEvent>>,
        deliver: std::sync::Mutex<Vec<(String, Option<String>)>>,
    }

    #[async_trait]
    impl SandboxRuntime for RecordingRuntime {
        fn live_available(&self) -> bool {
            true
        }

        async fn ingest(&self, event: BotEvent) -> Result<(), SandboxError> {
            self.ingest.lock().expect("ingest").push(event);
            Ok(())
        }

        async fn deliver(
            &self,
            _operation_id: &str,
            _conversation: &QqConversationRef,
            segments: &[MessageSegment],
            reply_to: Option<&str>,
        ) -> Result<serde_json::Value, SandboxError> {
            self.deliver
                .lock()
                .expect("deliver")
                .push((preview_segments(segments), reply_to.map(str::to_owned)));
            Ok(json!({ "delivered": true }))
        }
    }

    fn runtime() -> Arc<RecordingRuntime> {
        Arc::new(RecordingRuntime {
            ingest: std::sync::Mutex::new(Vec::new()),
            deliver: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn group(snapshot: &SandboxSnapshot) -> &SandboxConversationView {
        snapshot
            .conversations
            .iter()
            .find(|item| item.kind == BotConversationKind::Group)
            .expect("sandbox group")
    }

    async fn write(
        service: &SandboxService,
        revision: u64,
        operation_id: &str,
        action: SandboxAction,
    ) -> Result<SandboxWriteResult, SandboxError> {
        service
            .write(
                "tester",
                SandboxWriteRequest {
                    operation_id: operation_id.into(),
                    expected_revision: revision,
                    action,
                },
            )
            .await
    }

    fn now_ms() -> i64 {
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis(),
        )
        .unwrap_or(i64::MAX)
    }

    fn live_group_event(message_id: &str, text: &str, time_ms: i64) -> BotEvent {
        let mut message = BotMessage::text(
            BotTarget::Group {
                group_id: "group-1".into(),
            },
            text,
        );
        message.message_id = Some(message_id.into());
        BotEvent {
            event_id: format!("evt-{message_id}"),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "qq-main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms,
            target: BotTarget::Group {
                group_id: "group-1".into(),
            },
            actor: Some(BotUser {
                user_id: "member-1".into(),
                display_name: Some("群友甲".into()),
                avatar_url: Some("https://q.qlogo.cn/qqapp/APP_ID/member-1/640".into()),
            }),
            message: Some(message),
            raw: None,
            ext: BotExtMap::new(),
        }
    }

    fn live_group_toggle(event_type: &str, time_ms: i64) -> BotEvent {
        let mut ext = BotExtMap::new();
        ext.insert("qqbot.event_type".into(), json!(event_type));
        BotEvent {
            event_id: format!("evt-{event_type}"),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "qq-main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::PlatformSpecific(event_type.into()),
            time_ms,
            target: BotTarget::Group {
                group_id: "group-1".into(),
            },
            actor: None,
            message: None,
            raw: None,
            ext,
        }
    }

    async fn switch_live(service: &SandboxService) {
        write(
            service,
            service.snapshot("").await.unwrap().revision,
            "op-live",
            SandboxAction::SetMode {
                mode: SandboxMode::Live,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn simulate_ingests_user_message_and_rejects_bot_send() {
        let service = SandboxService::with_account("qq-main");
        let runtime = runtime();
        service.set_runtime(runtime.clone());

        let snapshot = service.snapshot("").await.unwrap();
        assert!(snapshot.flow_available);
        let group = group(&snapshot);
        let alice = sandbox_user_id("Alice");
        assert!(
            snapshot
                .conversations
                .iter()
                .any(|item| item.kind == BotConversationKind::Private
                    && item.conversation.user_id.as_deref() == Some(alice.as_str()))
        );
        write(
            &service,
            snapshot.revision,
            "op-1",
            SandboxAction::IngestAsUser {
                conversation_id: group.conversation_id.clone(),
                user_id: alice.clone(),
                text: "/ping".into(),
                segments: vec![],
                reply_to: None,
            },
        )
        .await
        .unwrap();
        {
            let ingested = runtime.ingest.lock().expect("ingest");
            assert_eq!(ingested.len(), 1);
            assert_eq!(ingested[0].kind, BotEventKind::MessageCreated);
            assert_eq!(ingested[0].platform, BotPlatform::QqBot);
            assert_eq!(ingested[0].message.as_ref().unwrap().plain_text(), "/ping");
            assert_eq!(
                ingested[0].ext.get("sandbox"),
                Some(&serde_json::Value::Bool(true))
            );
        }
        assert!(
            service
                .messages(&group.conversation_id)
                .await
                .unwrap()
                .iter()
                .any(|item| item.text == "/ping" && item.role == SandboxSpeakerRole::User)
        );

        let after = service.snapshot("").await.unwrap();
        let error = write(
            &service,
            after.revision,
            "op-bot",
            SandboxAction::SendAsBot {
                conversation_id: group.conversation_id.clone(),
                text: "不该发出".into(),
                segments: vec![],
                reply_to: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "invalid_state");
        assert!(runtime.deliver.lock().expect("deliver").is_empty());
    }

    #[tokio::test]
    async fn add_user_creates_private_chat_and_member_event() {
        let service = SandboxService::with_account("qq-main");
        let runtime = runtime();
        service.set_runtime(runtime.clone());
        let snapshot = service.snapshot("").await.unwrap();
        write(
            &service,
            snapshot.revision,
            "op-add",
            SandboxAction::AddUser,
        )
        .await
        .unwrap();
        let carol = sandbox_user_id("Carol");
        let after = service.snapshot("").await.unwrap();
        assert!(
            after
                .conversations
                .iter()
                .any(|item| item.conversation.user_id.as_deref() == Some(carol.as_str()))
        );
        assert!(
            group(&after)
                .users
                .iter()
                .any(|user| user.user_id == carol && user.display_name == "Carol")
        );
        {
            let events = runtime.ingest.lock().expect("ingest");
            assert_eq!(events[0].kind, BotEventKind::MemberJoined);
            assert_eq!(events[0].actor.as_ref().unwrap().user_id, carol);
        }

        write(
            &service,
            after.revision,
            "op-remove",
            SandboxAction::RemoveUser {
                user_id: carol.clone(),
            },
        )
        .await
        .unwrap();
        {
            let events = runtime.ingest.lock().expect("ingest");
            assert_eq!(events[1].kind, BotEventKind::MemberLeft);
            assert_eq!(events[1].actor.as_ref().unwrap().user_id, carol);
        }
        assert!(
            !service
                .snapshot("")
                .await
                .unwrap()
                .conversations
                .iter()
                .any(|item| item.conversation.user_id.as_deref() == Some(carol.as_str()))
        );
    }

    #[tokio::test]
    async fn write_rejects_stale_revision() {
        let service = SandboxService::with_account("qq-main");
        service.set_runtime(runtime());
        let snapshot = service.snapshot("").await.unwrap();
        write(
            &service,
            snapshot.revision,
            "op-bump",
            SandboxAction::AddUser,
        )
        .await
        .unwrap();
        let error = write(
            &service,
            snapshot.revision,
            "op-stale",
            SandboxAction::AddUser,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "revision.conflict");
    }

    #[tokio::test]
    async fn quote_round_trip_and_outbound_stays_in_sandbox() {
        let service = SandboxService::with_account("qq-main");
        service.set_runtime(runtime());
        let snapshot = service.snapshot("").await.unwrap();
        let group = group(&snapshot);
        let written = write(
            &service,
            snapshot.revision,
            "op-q1",
            SandboxAction::IngestAsUser {
                conversation_id: group.conversation_id.clone(),
                user_id: sandbox_user_id("Alice"),
                text: "hello".into(),
                segments: vec![],
                reply_to: None,
            },
        )
        .await
        .unwrap();
        let hello_id = written.result["message_id"].as_str().unwrap().to_owned();
        write(
            &service,
            written.revision,
            "op-q2",
            SandboxAction::IngestAsUser {
                conversation_id: group.conversation_id.clone(),
                user_id: sandbox_user_id("Bob"),
                text: "reply".into(),
                segments: vec![],
                reply_to: Some(hello_id.clone()),
            },
        )
        .await
        .unwrap();
        let quoted = service
            .messages(&group.conversation_id)
            .await
            .unwrap()
            .into_iter()
            .find(|item| item.text == "reply")
            .unwrap();
        assert_eq!(quoted.reply_to.as_deref(), Some(hello_id.as_str()));

        let outbound = service
            .observe_outbound(&group.conversation, &[MessageSegment::text("pong")], None)
            .unwrap();
        assert_eq!(outbound.role, SandboxSpeakerRole::Bot);
        assert_eq!(outbound.text, "pong");
        assert!(
            service
                .messages(&group.conversation_id)
                .await
                .unwrap()
                .iter()
                .any(|item| item.text == "pong" && item.role == SandboxSpeakerRole::Bot)
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn live_projects_inbound_and_sends_quoted_reply() {
        let service = SandboxService::with_account("qq-main");
        let runtime = runtime();
        service.set_runtime(runtime.clone());
        service.observe_event(live_group_event("qq-msg-1", "在吗", now_ms()));
        switch_live(&service).await;
        let live = service.snapshot("").await.unwrap();
        assert_eq!(live.mode, SandboxMode::Live);
        assert_eq!(live.conversations[0].users[0].display_name, "群友甲");
        assert_eq!(
            live.conversations[0].users[0].avatar_url.as_deref(),
            Some("https://q.qlogo.cn/qqapp/APP_ID/member-1/640")
        );
        assert_eq!(live.conversations[0].avatar_url, None);
        let conversation_id = live.conversations[0].conversation_id.clone();
        let live_messages = service.messages(&conversation_id).await.unwrap();
        assert_eq!(live_messages[0].role, SandboxSpeakerRole::User);
        assert_eq!(live_messages[0].message_id, "qq-msg-1");

        service.observe_outbound(
            &QqConversationRef {
                version: QQ_CONVERSATION_REF_VERSION,
                account_id: "qq-main".into(),
                kind: BotConversationKind::Group,
                user_id: None,
                group_id: Some("group-1".into()),
                guild_id: None,
                channel_id: None,
                thread_id: None,
            },
            &[MessageSegment::text("机器人自己")],
            None,
        );
        assert!(
            service
                .messages(&conversation_id)
                .await
                .unwrap()
                .iter()
                .any(|item| item.text == "机器人自己" && item.role == SandboxSpeakerRole::Bot)
        );

        write(
            &service,
            service.snapshot("").await.unwrap().revision,
            "op-send",
            SandboxAction::SendAsBot {
                conversation_id: conversation_id.clone(),
                text: "后台回复".into(),
                segments: vec![],
                reply_to: Some("qq-msg-1".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            runtime.deliver.lock().expect("deliver").as_slice(),
            [("后台回复".into(), Some("qq-msg-1".into()))]
        );

        let error = write(
            &service,
            service.snapshot("").await.unwrap().revision,
            "op-forge",
            SandboxAction::IngestAsUser {
                conversation_id,
                user_id: sandbox_user_id("Alice"),
                text: "伪造".into(),
                segments: vec![],
                reply_to: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "invalid_state");
    }

    #[tokio::test]
    async fn live_send_uses_active_message_when_group_enabled() {
        let service = SandboxService::with_account("qq-main");
        let runtime = runtime();
        service.set_runtime(runtime.clone());
        service.observe_event(live_group_event("qq-msg-1", "在吗", now_ms()));
        switch_live(&service).await;
        let live = service.snapshot("").await.unwrap();
        assert!(live.conversations[0].active_message);
        let conversation_id = live.conversations[0].conversation_id.clone();
        write(
            &service,
            live.revision,
            "op-active",
            SandboxAction::SendAsBot {
                conversation_id: conversation_id.clone(),
                text: "主动推送".into(),
                segments: vec![],
                reply_to: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            runtime.deliver.lock().expect("deliver").as_slice(),
            [("主动推送".into(), None)]
        );

        service.observe_event(live_group_toggle("GROUP_MSG_REJECT", now_ms()));
        let denied = write(
            &service,
            service.snapshot("").await.unwrap().revision,
            "op-denied",
            SandboxAction::SendAsBot {
                conversation_id,
                text: "再推一次".into(),
                segments: vec![],
                reply_to: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(denied.code, "invalid_argument");
        assert!(denied.message.contains("主动消息权限"));
    }

    #[tokio::test]
    async fn live_send_accepts_markdown_and_keyboard() {
        let service = SandboxService::with_account("qq-main");
        let runtime = runtime();
        service.set_runtime(runtime.clone());
        service.observe_event(live_group_event("qq-msg-1", "在吗", now_ms()));
        switch_live(&service).await;
        let live = service.snapshot("").await.unwrap();
        write(
            &service,
            live.revision,
            "op-md",
            SandboxAction::SendAsBot {
                conversation_id: live.conversations[0].conversation_id.clone(),
                text: String::new(),
                segments: vec![
                    MessageSegment::markdown("# 签到"),
                    MessageSegment::platform_specific(
                        "qqbot",
                        "keyboard",
                        json!({ "content": { "rows": [] } }),
                    ),
                ],
                reply_to: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            runtime.deliver.lock().expect("deliver").as_slice(),
            [("[Markdown][按钮]".into(), None)]
        );
    }

    #[tokio::test]
    async fn sandbox_bot_profile_appears_in_snapshot_and_outbound() {
        let service = SandboxService::with_account("qq-main");
        assert_eq!(service.snapshot("").await.unwrap().bot, None);
        service.observe_event(BotEvent {
            event_id: "ready".into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "qq-main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::BotConnected,
            time_ms: now_ms(),
            target: BotTarget::Group {
                group_id: "group-1".into(),
            },
            actor: Some(BotUser {
                user_id: "BOT_OPENID".into(),
                display_name: Some("mutsuki".into()),
                avatar_url: Some("https://q.qlogo.cn/qqapp/APP_ID/BOT_OPENID/640".into()),
            }),
            message: None,
            raw: None,
            ext: BotExtMap::new(),
        });
        let snapshot = service.snapshot("").await.unwrap();
        let bot = snapshot.bot.as_ref().expect("bot profile");
        assert_eq!(bot.user_id, "BOT_OPENID");
        assert_eq!(bot.display_name.as_deref(), Some("mutsuki"));
        assert_eq!(
            bot.avatar_url.as_deref(),
            Some("https://q.qlogo.cn/qqapp/APP_ID/BOT_OPENID/640")
        );

        let conversation = group(&snapshot).conversation.clone();
        let conversation_id = group(&snapshot).conversation_id.clone();
        service.observe_outbound(&conversation, &[MessageSegment::text("机器人自己")], None);
        let bot_message = service
            .messages(&conversation_id)
            .await
            .unwrap()
            .into_iter()
            .rev()
            .find(|item| item.role == SandboxSpeakerRole::Bot)
            .unwrap();
        assert_eq!(bot_message.sender_id, "BOT_OPENID");
        assert_eq!(bot_message.sender_name, "mutsuki");
        assert_eq!(bot_message.text, "机器人自己");
    }

    #[tokio::test]
    async fn live_send_rejects_bot_unknown_and_expired_quotes() {
        let service = SandboxService::with_account("qq-main");
        service.set_runtime(runtime());
        service.observe_event(live_group_event("qq-msg-fresh", "在吗", now_ms()));
        switch_live(&service).await;
        let conversation_id = service.snapshot("").await.unwrap().conversations[0]
            .conversation_id
            .clone();
        service.observe_outbound(
            &QqConversationRef {
                version: QQ_CONVERSATION_REF_VERSION,
                account_id: "qq-main".into(),
                kind: BotConversationKind::Group,
                user_id: None,
                group_id: Some("group-1".into()),
                guild_id: None,
                channel_id: None,
                thread_id: None,
            },
            &[MessageSegment::text("机器人自己")],
            None,
        );
        let bot_id = service
            .messages(&conversation_id)
            .await
            .unwrap()
            .into_iter()
            .find(|item| item.role == SandboxSpeakerRole::Bot)
            .unwrap()
            .message_id;

        let bot_quote = write(
            &service,
            service.snapshot("").await.unwrap().revision,
            "op-bot-quote",
            SandboxAction::SendAsBot {
                conversation_id: conversation_id.clone(),
                text: "不能引用机器人".into(),
                segments: vec![],
                reply_to: Some(bot_id),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(bot_quote.code, "invalid_argument");
        assert!(bot_quote.message.contains("用户消息"));

        let unknown = write(
            &service,
            service.snapshot("").await.unwrap().revision,
            "op-unknown",
            SandboxAction::SendAsBot {
                conversation_id: conversation_id.clone(),
                text: "未知引用".into(),
                segments: vec![],
                reply_to: Some("missing-id".into()),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(unknown.code, "invalid_argument");
        assert!(unknown.message.contains("不存在"));

        service.observe_event(live_group_event(
            "qq-msg-old",
            "很久以前",
            now_ms() - (6 * 60 * 1_000),
        ));
        let expired = write(
            &service,
            service.snapshot("").await.unwrap().revision,
            "op-expired",
            SandboxAction::SendAsBot {
                conversation_id,
                text: "过期回复".into(),
                segments: vec![],
                reply_to: Some("qq-msg-old".into()),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(expired.code, "invalid_argument");
        assert!(expired.message.contains("5 分钟"));
    }

    #[tokio::test]
    async fn live_send_keeps_passive_delivery_and_rejects_failed_receipt() {
        struct FailedDeliveryRuntime;

        #[async_trait]
        impl SandboxRuntime for FailedDeliveryRuntime {
            fn live_available(&self) -> bool {
                true
            }

            async fn ingest(&self, _event: BotEvent) -> Result<(), SandboxError> {
                Ok(())
            }

            async fn deliver(
                &self,
                _operation_id: &str,
                _conversation: &QqConversationRef,
                _segments: &[MessageSegment],
                _reply_to: Option<&str>,
            ) -> Result<serde_json::Value, SandboxError> {
                Err(SandboxError::new(
                    "qqbot.openapi.permanent",
                    "真实消息发送失败（qqbot.openapi.permanent）",
                ))
            }
        }

        let service = SandboxService::with_account("qq-main");
        service.set_runtime(Arc::new(FailedDeliveryRuntime));
        service.observe_event(live_group_event("qq-msg-1", "在吗", now_ms()));
        switch_live(&service).await;
        let live = service.snapshot("").await.unwrap();
        let error = write(
            &service,
            live.revision,
            "op-send",
            SandboxAction::SendAsBot {
                conversation_id: live.conversations[0].conversation_id.clone(),
                text: "后台回复".into(),
                segments: vec![],
                reply_to: Some("qq-msg-1".into()),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "qqbot.openapi.permanent");
        assert!(error.message.contains("真实消息发送失败"));
        assert!(
            service
                .messages(&live.conversations[0].conversation_id)
                .await
                .unwrap()
                .iter()
                .all(|item| item.role != SandboxSpeakerRole::Bot)
        );
    }

    #[test]
    fn sandbox_identity_uses_reserved_prefix() {
        assert!(is_sandbox_id(&sandbox_user_id("Alice")));
        assert!(is_sandbox_target(&BotTarget::Group {
            group_id: SANDBOX_GROUP_ID.into(),
        }));
        assert!(!is_sandbox_target(&BotTarget::Group {
            group_id: "group-1".into(),
        }));
    }

    #[tokio::test]
    async fn update_user_renames_openid_and_private_chat() {
        let service = SandboxService::with_account("qq-main");
        let runtime = runtime();
        service.set_runtime(runtime.clone());
        let snapshot = service.snapshot("").await.unwrap();
        let alice = sandbox_user_id("Alice");
        write(
            &service,
            snapshot.revision,
            "op-rename",
            SandboxAction::UpdateUser {
                user_id: alice.clone(),
                new_user_id: "real-openid".into(),
                display_name: "阿狸".into(),
            },
        )
        .await
        .unwrap();
        let after = service.snapshot("").await.unwrap();
        assert!(
            group(&after)
                .users
                .iter()
                .any(|user| user.user_id == "real-openid" && user.display_name == "阿狸")
        );
        assert!(after.conversations.iter().any(|item| {
            item.kind == BotConversationKind::Private
                && item.conversation.user_id.as_deref() == Some("real-openid")
                && item.title == "阿狸"
        }));
        assert!(
            !after
                .conversations
                .iter()
                .any(|item| item.conversation.user_id.as_deref() == Some(alice.as_str()))
        );
        let events = runtime.ingest.lock().expect("ingest");
        assert_eq!(events[0].kind, BotEventKind::MemberLeft);
        assert_eq!(events[0].actor.as_ref().unwrap().user_id, alice);
        assert_eq!(events[1].kind, BotEventKind::MemberJoined);
        assert_eq!(events[1].actor.as_ref().unwrap().user_id, "real-openid");
        assert_eq!(
            events[1].actor.as_ref().unwrap().display_name.as_deref(),
            Some("阿狸")
        );
    }

    #[tokio::test]
    async fn import_live_users_copies_openid_and_nickname() {
        let service = SandboxService::with_account("qq-main");
        let runtime = runtime();
        service.set_runtime(runtime.clone());
        service.observe_event(live_group_event("live-member", "在吗", 1_700_000_000_000));
        switch_live(&service).await;
        let live = service.snapshot("").await.unwrap();
        assert_eq!(live.mode, SandboxMode::Live);
        assert!(live.live_users.iter().any(|user| user.user_id == "member-1"
            && user.display_name == "群友甲"
            && user.avatar_url.as_deref() == Some("https://q.qlogo.cn/qqapp/APP_ID/member-1/640")));
        write(
            &service,
            live.revision,
            "op-import",
            SandboxAction::ImportLiveUsers {
                user_ids: vec!["member-1".into()],
            },
        )
        .await
        .unwrap();
        write(
            &service,
            service.snapshot("").await.unwrap().revision,
            "op-simulate",
            SandboxAction::SetMode {
                mode: SandboxMode::Simulate,
            },
        )
        .await
        .unwrap();
        let after = service.snapshot("").await.unwrap();
        assert!(
            group(&after)
                .users
                .iter()
                .any(|user| user.user_id == "member-1"
                    && user.display_name == "群友甲"
                    && user.avatar_url.as_deref()
                        == Some("https://q.qlogo.cn/qqapp/APP_ID/member-1/640"))
        );
        assert!(after.conversations.iter().any(|item| {
            item.kind == BotConversationKind::Private
                && item.conversation.user_id.as_deref() == Some("member-1")
        }));
        let skipped = write(
            &service,
            after.revision,
            "op-import-again",
            SandboxAction::ImportLiveUsers {
                user_ids: vec!["member-1".into()],
            },
        )
        .await
        .unwrap();
        assert!(skipped.result["imported"].as_array().unwrap().is_empty());
        assert_eq!(skipped.result["skipped"][0]["reason"], "already_exists");
    }

    #[test]
    fn parse_sandbox_mentions_matches_roster_and_all() {
        let users = vec![
            SandboxUserView {
                user_id: sandbox_user_id("Alice"),
                display_name: "Alice".into(),
                avatar_url: None,
                last_seen_unix_ms: 0,
                message_count: 0,
            },
            SandboxUserView {
                user_id: sandbox_user_id("Bob"),
                display_name: "Bob".into(),
                avatar_url: None,
                last_seen_unix_ms: 0,
                message_count: 0,
            },
        ];
        let segments = parse_sandbox_mentions("hi @Alice and @全体成员", &users);
        assert!(matches!(
            &segments[0],
            MessageSegment::Text { text } if text == "hi "
        ));
        assert!(matches!(
            &segments[1],
            MessageSegment::MentionUser { user_id } if user_id == &sandbox_user_id("Alice")
        ));
        assert!(matches!(
            &segments[2],
            MessageSegment::Text { text } if text == " and "
        ));
        assert!(matches!(&segments[3], MessageSegment::MentionAll));
    }

    #[tokio::test]
    async fn simulate_ingests_mentions_media_and_ark() {
        let service = SandboxService::with_account("qq-main");
        service.set_runtime(runtime());
        let snapshot = service.snapshot("").await.unwrap();
        let group = group(&snapshot);
        write(
            &service,
            snapshot.revision,
            "op-at",
            SandboxAction::IngestAsUser {
                conversation_id: group.conversation_id.clone(),
                user_id: sandbox_user_id("Alice"),
                text: "hello @Bob".into(),
                segments: vec![],
                reply_to: None,
            },
        )
        .await
        .unwrap();
        let mentioned = service
            .messages(&group.conversation_id)
            .await
            .unwrap()
            .into_iter()
            .find(|item| item.text.contains("@sandbox:bob") || item.text.contains("@Bob"))
            .unwrap();
        assert!(mentioned.refs.iter().any(|item| {
            item.kind == SandboxRefKind::Mention
                && item.id.as_deref() == Some(&sandbox_user_id("Bob"))
        }));

        let uploaded = service
            .upload_media("pic.png", "image/png", b"fake-png".to_vec())
            .await
            .unwrap();
        let after = service.snapshot("").await.unwrap();
        write(
            &service,
            after.revision,
            "op-rich",
            SandboxAction::IngestAsUser {
                conversation_id: group.conversation_id.clone(),
                user_id: sandbox_user_id("Alice"),
                text: String::new(),
                segments: vec![
                    MessageSegment::PlatformSpecific {
                        platform: "sandbox".into(),
                        kind: "media".into(),
                        payload: json!({
                            "media_id": uploaded.media_id,
                            "mime": "image/png",
                            "name": "pic.png"
                        }),
                    },
                    MessageSegment::PlatformSpecific {
                        platform: "qqbot".into(),
                        kind: "ark".into(),
                        payload: json!({
                            "template_id": 23,
                            "kv": [{"key": "#METATITLE#", "value": "卡片"}]
                        }),
                    },
                ],
                reply_to: None,
            },
        )
        .await
        .unwrap();
        let blob = service.media_blob(&uploaded.media_id).await.unwrap();
        assert_eq!(blob.bytes, b"fake-png");
        assert!(uploaded.media_id.starts_with("sha256:"));
        let again = service
            .upload_media("pic.png", "image/png", b"fake-png".to_vec())
            .await
            .unwrap();
        assert_eq!(again.media_id, uploaded.media_id);
        let rich = service
            .messages(&group.conversation_id)
            .await
            .unwrap()
            .into_iter()
            .find(|item| {
                item.refs
                    .iter()
                    .any(|item| item.kind == SandboxRefKind::Ark)
            })
            .unwrap();
        assert!(rich.refs.iter().any(|item| item.kind == SandboxRefKind::Img
            && item.h.as_deref() == Some(uploaded.media_id.as_str())));
    }

    #[tokio::test]
    async fn simulate_ingests_markdown_and_keyboard() {
        let service = SandboxService::with_account("qq-main");
        service.set_runtime(runtime());
        let snapshot = service.snapshot("").await.unwrap();
        let group = group(&snapshot);
        write(
            &service,
            snapshot.revision,
            "op-md",
            SandboxAction::IngestAsUser {
                conversation_id: group.conversation_id.clone(),
                user_id: sandbox_user_id("Alice"),
                text: String::new(),
                segments: vec![
                    MessageSegment::markdown("**hi**"),
                    MessageSegment::platform_specific(
                        "qqbot",
                        "keyboard",
                        json!({
                            "content": {
                                "rows": [{
                                    "buttons": [{
                                        "id": "btn_1",
                                        "render_data": { "label": "签到" },
                                        "action": { "type": 2, "data": "/签到" }
                                    }]
                                }]
                            }
                        }),
                    ),
                ],
                reply_to: None,
            },
        )
        .await
        .unwrap();
        let message = service
            .messages(&group.conversation_id)
            .await
            .unwrap()
            .into_iter()
            .find(|item| {
                item.refs
                    .iter()
                    .any(|item| item.kind == SandboxRefKind::Markdown)
            })
            .unwrap();
        assert!(message.refs.iter().any(|item| {
            item.kind == SandboxRefKind::Markdown
                && item
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("content"))
                    .and_then(Value::as_str)
                    == Some("**hi**")
        }));
        assert!(
            message
                .refs
                .iter()
                .any(|item| item.kind == SandboxRefKind::Keyboard)
        );
        let mixed = write(
            &service,
            service.snapshot("").await.unwrap().revision,
            "op-mix",
            SandboxAction::IngestAsUser {
                conversation_id: group.conversation_id.clone(),
                user_id: sandbox_user_id("Alice"),
                text: "plain".into(),
                segments: vec![MessageSegment::markdown("# no")],
                reply_to: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(mixed.code, "invalid_argument");
    }

    #[tokio::test]
    async fn stickers_and_faces_stay_separate_from_media() {
        let store = Arc::new(MemoryHistoryStore::default());
        let service = SandboxService::with_history("qq-main", store.clone()).unwrap();
        service.set_runtime(runtime());
        let sticker = service
            .upload_sticker("pack.png", "image/png", b"sticker-bytes".to_vec())
            .await
            .unwrap();
        let media = service
            .upload_media("pic.png", "image/png", b"image-bytes".to_vec())
            .await
            .unwrap();
        assert_ne!(sticker.media_id, media.media_id);
        assert!(service.media_blob(&sticker.media_id).await.is_err());
        assert!(service.sticker_blob(&media.media_id).await.is_err());
        let blob = service.sticker_blob(&sticker.media_id).await.unwrap();
        assert_eq!(blob.bytes, b"sticker-bytes");

        let snapshot = service.snapshot("").await.unwrap();
        write(
            &service,
            snapshot.revision,
            "op-sticker",
            SandboxAction::IngestAsUser {
                conversation_id: group(&snapshot).conversation_id.clone(),
                user_id: sandbox_user_id("Alice"),
                text: String::new(),
                segments: vec![MessageSegment::PlatformSpecific {
                    platform: "sandbox".into(),
                    kind: "sticker".into(),
                    payload: json!({
                        "sticker_id": sticker.media_id,
                        "mime": "image/png",
                        "name": "pack.png"
                    }),
                }],
                reply_to: None,
            },
        )
        .await
        .unwrap();
        let sent = service
            .messages(&group(&service.snapshot("").await.unwrap()).conversation_id)
            .await
            .unwrap()
            .into_iter()
            .find(|item| {
                item.refs
                    .iter()
                    .any(|item| item.kind == SandboxRefKind::Sticker)
            })
            .unwrap();
        assert!(
            sent.refs
                .iter()
                .any(|item| item.kind == SandboxRefKind::Sticker
                    && item.h.as_deref() == Some(sticker.media_id.as_str()))
        );

        let mut face_event = live_group_event("qq-face-1", "", now_ms());
        if let Some(message) = face_event.message.as_mut() {
            message.segments = vec![MessageSegment::PlatformSpecific {
                platform: "qqbot".into(),
                kind: "face".into(),
                payload: json!({ "face_type": "6", "face_id": "0" }),
            }];
        }
        service.observe_event(face_event);

        let listed = service.list_stickers().await.unwrap();
        assert!(listed.iter().any(|item| {
            item.kind == SandboxStickerKind::Custom && item.id == sticker.media_id
        }));
        assert!(listed.iter().any(|item| {
            item.kind == SandboxStickerKind::QqFace
                && item.id == "qq:6:0"
                && item.face_type.as_deref() == Some("6")
                && item.face_id.as_deref() == Some("0")
        }));
        let persisted = store.load().unwrap();
        assert!(
            persisted
                .media
                .iter()
                .all(|asset| asset.content_hash != sticker.media_id)
        );
        assert!(
            persisted
                .stickers
                .iter()
                .any(|item| item.content_hash == sticker.media_id
                    && item.bytes == b"sticker-bytes")
        );
        assert!(
            persisted
                .faces
                .iter()
                .any(|item| item.face_key == "qq:6:0" && item.face_type == "6")
        );
    }

    #[tokio::test]
    async fn live_deliver_forwards_mention_segments() {
        let service = SandboxService::with_account("qq-main");
        let runtime = runtime();
        service.set_runtime(runtime.clone());
        service.observe_event(live_group_event("qq-msg-1", "在吗", now_ms()));
        switch_live(&service).await;
        let conversation_id = service.snapshot("").await.unwrap().conversations[0]
            .conversation_id
            .clone();
        write(
            &service,
            service.snapshot("").await.unwrap().revision,
            "op-mention",
            SandboxAction::SendAsBot {
                conversation_id,
                text: String::new(),
                segments: vec![
                    MessageSegment::text("hi "),
                    MessageSegment::MentionUser {
                        user_id: "member-1".into(),
                    },
                ],
                reply_to: Some("qq-msg-1".into()),
            },
        )
        .await
        .unwrap();
        let delivered = runtime.deliver.lock().expect("deliver");
        assert_eq!(delivered[0].0, "hi @member-1");
        assert_eq!(delivered[0].1.as_deref(), Some("qq-msg-1"));
    }

    #[derive(Default)]
    struct MemoryHistoryStore {
        inner: std::sync::Mutex<SandboxHistorySnapshot>,
    }

    impl SandboxHistoryStore for MemoryHistoryStore {
        fn load(&self) -> Result<SandboxHistorySnapshot, SandboxError> {
            Ok(self.inner.lock().expect("history").clone())
        }

        fn save(&self, snapshot: &SandboxHistorySnapshot) -> Result<(), SandboxError> {
            *self.inner.lock().expect("history") = snapshot.clone();
            Ok(())
        }
    }

    #[tokio::test]
    async fn history_store_restores_simulate_and_live_after_restart() {
        let store = Arc::new(MemoryHistoryStore::default());
        let first = SandboxService::with_history("qq-main", store.clone()).unwrap();
        first.set_runtime(runtime());
        let snapshot = first.snapshot("").await.unwrap();
        assert!(
            group(&snapshot)
                .users
                .iter()
                .any(|user| user.display_name == "Alice")
        );
        write(
            &first,
            snapshot.revision,
            "op-seed-msg",
            SandboxAction::IngestAsUser {
                conversation_id: group(&snapshot).conversation_id.clone(),
                user_id: sandbox_user_id("Alice"),
                text: "hello history".into(),
                segments: vec![],
                reply_to: None,
            },
        )
        .await
        .unwrap();
        first.observe_event(live_group_event("qq-msg-hist", "在吗", now_ms()));
        write(
            &first,
            first.snapshot("").await.unwrap().revision,
            "op-live",
            SandboxAction::SetMode {
                mode: SandboxMode::Live,
            },
        )
        .await
        .unwrap();

        let restored = SandboxService::with_history("qq-main", store).unwrap();
        let live = restored.snapshot("").await.unwrap();
        assert_eq!(live.mode, SandboxMode::Live);
        assert!(
            restored
                .messages(&live.conversations[0].conversation_id)
                .await
                .unwrap()
                .iter()
                .any(|item| item.message_id == "qq-msg-hist")
        );
        write(
            &restored,
            live.revision,
            "op-sim",
            SandboxAction::SetMode {
                mode: SandboxMode::Simulate,
            },
        )
        .await
        .unwrap();
        let simulate = restored.snapshot("").await.unwrap();
        assert!(
            restored
                .messages(&group(&simulate).conversation_id)
                .await
                .unwrap()
                .iter()
                .any(|item| item.text == "hello history")
        );
    }

    #[tokio::test]
    async fn history_store_does_not_reseed_existing_simulate() {
        let store = Arc::new(MemoryHistoryStore::default());
        let first = SandboxService::with_history("qq-main", store.clone()).unwrap();
        first.set_runtime(runtime());
        let snapshot = first.snapshot("").await.unwrap();
        write(&first, snapshot.revision, "op-add", SandboxAction::AddUser)
            .await
            .unwrap();
        drop(first);
        let restored = SandboxService::with_history("qq-main", store).unwrap();
        let after = restored.snapshot("").await.unwrap();
        assert!(
            group(&after)
                .users
                .iter()
                .any(|user| user.display_name == "Carol")
        );
    }

    #[tokio::test]
    async fn live_message_upsert_is_idempotent() {
        let store = Arc::new(MemoryHistoryStore::default());
        let service = SandboxService::with_history("qq-main", store).unwrap();
        let event = live_group_event("qq-dup", "重复", now_ms());
        service.observe_event(event.clone());
        service.observe_event(event);
        switch_live(&service).await;
        let conversation_id = service.snapshot("").await.unwrap().conversations[0]
            .conversation_id
            .clone();
        let messages = service.messages(&conversation_id).await.unwrap();
        assert_eq!(
            messages
                .iter()
                .filter(|item| item.message_id == "qq-dup")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn live_same_image_hash_dedups_and_refreshes_url() {
        let store = Arc::new(MemoryHistoryStore::default());
        let service = SandboxService::with_history("qq-main", store.clone()).unwrap();
        let hash = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        service.observe_event(live_image_event(
            "qq-img-1",
            "https://cdn.example/old.png",
            "ref-old",
            hash,
            now_ms(),
        ));
        service.observe_event(live_image_event(
            "qq-img-2",
            "https://cdn.example/new.png",
            "ref-new",
            hash,
            now_ms(),
        ));
        let snapshot = store.load().unwrap();
        assert_eq!(snapshot.media.len(), 1);
        assert_eq!(
            snapshot.media[0].url.as_deref(),
            Some("https://cdn.example/new.png")
        );
        switch_live(&service).await;
        let conversation_id = service.snapshot("").await.unwrap().conversations[0]
            .conversation_id
            .clone();
        let messages = service.messages(&conversation_id).await.unwrap();
        let hashed = messages
            .iter()
            .filter(|item| item.refs.iter().any(|item| item.h.as_deref() == Some(hash)))
            .count();
        assert_eq!(hashed, 2);
        assert_eq!(
            messages
                .iter()
                .rev()
                .find(|item| item.message_id == "qq-img-2")
                .unwrap()
                .refs[0]
                .url
                .as_deref(),
            Some("https://cdn.example/new.png")
        );
    }

    #[tokio::test]
    async fn truncated_messages_release_unreferenced_assets() {
        let store = Arc::new(MemoryHistoryStore::default());
        let service = SandboxService::with_history("qq-main", store.clone()).unwrap();
        service.set_runtime(runtime());
        let snapshot = service.snapshot("").await.unwrap();
        let conversation_id = group(&snapshot).conversation_id.clone();
        let user_id = sandbox_user_id("Alice");
        let mut revision = snapshot.revision;
        let mut first_hash = String::new();
        let overflow = SANDBOX_MAX_MESSAGES + SANDBOX_MAX_MEDIA_ITEMS + 1;
        for index in 0..overflow {
            let uploaded = service
                .upload_media("pic.png", "image/png", format!("blob-{index}").into_bytes())
                .await
                .unwrap();
            if index == 0 {
                first_hash = uploaded.media_id.clone();
            }
            revision = write(
                &service,
                revision,
                &format!("op-gc-{index}"),
                SandboxAction::IngestAsUser {
                    conversation_id: conversation_id.clone(),
                    user_id: user_id.clone(),
                    text: String::new(),
                    segments: vec![MessageSegment::PlatformSpecific {
                        platform: "sandbox".into(),
                        kind: "media".into(),
                        payload: json!({
                            "media_id": uploaded.media_id,
                            "mime": "image/png",
                            "name": "pic.png"
                        }),
                    }],
                    reply_to: None,
                },
            )
            .await
            .unwrap()
            .revision;
        }
        let messages = service.messages(&conversation_id).await.unwrap();
        assert_eq!(messages.len(), SANDBOX_MAX_MESSAGES);
        assert!(messages.iter().all(|item| {
            item.refs
                .iter()
                .all(|item| item.h.as_deref() != Some(first_hash.as_str()))
        }));
        let media = store.load().unwrap().media;
        assert!(media.iter().all(|asset| asset.content_hash != first_hash));
        assert_eq!(media.len(), SANDBOX_MAX_MESSAGES + SANDBOX_MAX_MEDIA_ITEMS);
    }

    fn live_image_event(
        message_id: &str,
        url: &str,
        ref_id: &str,
        hash: &str,
        time_ms: i64,
    ) -> BotEvent {
        let mut event = live_group_event(message_id, "", time_ms);
        if let Some(message) = event.message.as_mut() {
            message.segments = vec![
                MessageSegment::PlatformSpecific {
                    platform: "qqbot".into(),
                    kind: "attachment".into(),
                    payload: json!({
                        "url": url,
                        "content_type": "image/png",
                        "filename": "a.png"
                    }),
                },
                MessageSegment::Image {
                    resource: ResourceRef {
                        ref_id: ref_id.into(),
                        resource_id: ResourceId {
                            kind_id: "blob".into(),
                            slot_id: hash.into(),
                            generation: 1,
                            version: 1,
                        },
                        semantic: ResourceSemantic::FrozenValue,
                        provider_id: "test".into(),
                        resource_kind: "blob".into(),
                        schema: "image/png".into(),
                        version: 1,
                        generation: 1,
                        access: ResourceAccess::Inline,
                        size_hint: Some(1),
                        content_hash: Some(hash.into()),
                        lifetime: ResourceLifetime::Persistent,
                        lease: None,
                        seal_state: ResourceSealState::Sealed,
                    },
                },
            ];
        }
        event
    }
}
