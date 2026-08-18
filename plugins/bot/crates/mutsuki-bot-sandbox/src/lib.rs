//! Headless QQ conversation sandbox used by the Bot Web Console.

mod service;
mod types;

pub use service::{SandboxApi, SandboxChangeSubscription, SandboxRuntime, SandboxService};
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
    use serde_json::json;

    use super::*;

    struct RecordingRuntime {
        ingest: std::sync::Mutex<Vec<BotEvent>>,
        deliver: std::sync::Mutex<Vec<String>>,
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
            text: &str,
        ) -> Result<serde_json::Value, SandboxError> {
            self.deliver.lock().expect("deliver").push(text.into());
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
        assert!(
            quoted
                .segments
                .iter()
                .any(|segment| matches!(segment, MessageSegment::Quote { message_id } if message_id == &hello_id))
        );

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
    async fn live_projects_inbound_and_sends_as_bot() {
        let service = SandboxService::with_account("qq-main");
        let runtime = runtime();
        service.set_runtime(runtime.clone());
        service.observe_event(BotEvent {
            event_id: "evt-1".into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "qq-main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1_700_000_000_000,
            target: BotTarget::Group {
                group_id: "group-1".into(),
            },
            actor: Some(BotUser {
                user_id: "member-1".into(),
                display_name: Some("群友甲".into()),
                avatar_url: None,
            }),
            message: Some(BotMessage::text(
                BotTarget::Group {
                    group_id: "group-1".into(),
                },
                "在吗",
            )),
            raw: None,
            ext: BotExtMap::new(),
        });
        let after_observe = service.snapshot("").await.unwrap();
        write(
            &service,
            after_observe.revision,
            "op-live",
            SandboxAction::SetMode {
                mode: SandboxMode::Live,
            },
        )
        .await
        .unwrap();
        let live = service.snapshot("").await.unwrap();
        assert_eq!(live.mode, SandboxMode::Live);
        assert_eq!(live.conversations[0].users[0].display_name, "群友甲");
        let live_messages = service
            .messages(&live.conversations[0].conversation_id)
            .await
            .unwrap();
        assert_eq!(live_messages[0].role, SandboxSpeakerRole::User);

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
                .messages(&live.conversations[0].conversation_id)
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
                conversation_id: live.conversations[0].conversation_id.clone(),
                text: "后台回复".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            runtime.deliver.lock().expect("deliver").as_slice(),
            ["后台回复"]
        );

        let error = write(
            &service,
            service.snapshot("").await.unwrap().revision,
            "op-forge",
            SandboxAction::IngestAsUser {
                conversation_id: live.conversations[0].conversation_id.clone(),
                user_id: sandbox_user_id("Alice"),
                text: "伪造".into(),
                reply_to: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "invalid_state");
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
        service.observe_event(BotEvent {
            event_id: "evt-live-member".into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "qq-main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1_700_000_000_000,
            target: BotTarget::Group {
                group_id: "group-1".into(),
            },
            actor: Some(BotUser {
                user_id: "member-1".into(),
                display_name: Some("群友甲".into()),
                avatar_url: None,
            }),
            message: Some(BotMessage::text(
                BotTarget::Group {
                    group_id: "group-1".into(),
                },
                "在吗",
            )),
            raw: None,
            ext: BotExtMap::new(),
        });
        let snapshot = service.snapshot("").await.unwrap();
        assert_eq!(snapshot.mode, SandboxMode::Simulate);
        assert!(
            snapshot
                .live_users
                .iter()
                .any(|user| user.user_id == "member-1" && user.display_name == "群友甲")
        );
        write(
            &service,
            snapshot.revision,
            "op-import",
            SandboxAction::ImportLiveUsers {
                user_ids: vec!["member-1".into()],
            },
        )
        .await
        .unwrap();
        let after = service.snapshot("").await.unwrap();
        assert!(
            group(&after)
                .users
                .iter()
                .any(|user| user.user_id == "member-1" && user.display_name == "群友甲")
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
}
