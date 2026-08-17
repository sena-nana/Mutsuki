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
        BotPlatform, BotTarget, BotUser,
    };
    use serde_json::json;

    use super::*;

    struct RecordingRuntime {
        ingest: std::sync::Mutex<Vec<String>>,
        deliver: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl SandboxRuntime for RecordingRuntime {
        fn live_available(&self) -> bool {
            true
        }

        async fn ingest(&self, event: BotEvent) -> Result<(), SandboxError> {
            self.ingest
                .lock()
                .expect("ingest")
                .push(event.message.as_ref().unwrap().plain_text());
            Ok(())
        }

        async fn deliver(
            &self,
            _operation_id: &str,
            _conversation: &mutsuki_bot_protocol::QqConversationRef,
            text: &str,
        ) -> Result<serde_json::Value, SandboxError> {
            self.deliver.lock().expect("deliver").push(text.into());
            Ok(json!({ "delivered": true }))
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn simulate_and_live_round_trip() {
        let service = SandboxService::with_account("qq-main");
        let runtime = Arc::new(RecordingRuntime {
            ingest: std::sync::Mutex::new(Vec::new()),
            deliver: std::sync::Mutex::new(Vec::new()),
        });
        service.set_runtime(runtime.clone());

        let snapshot = service.snapshot("").await.unwrap();
        let group = snapshot
            .conversations
            .iter()
            .find(|item| item.kind == BotConversationKind::Group)
            .unwrap();
        service
            .write(
                "tester",
                SandboxWriteRequest {
                    operation_id: "op-1".into(),
                    expected_revision: snapshot.revision,
                    action: SandboxAction::IngestAsUser {
                        conversation_id: group.conversation_id.clone(),
                        user_id: "alice".into(),
                        text: "/ping".into(),
                        inject_into_flow: true,
                    },
                },
            )
            .await
            .unwrap();
        assert_eq!(runtime.ingest.lock().expect("ingest").as_slice(), ["/ping"]);
        assert!(
            service
                .messages(&group.conversation_id)
                .await
                .unwrap()
                .iter()
                .any(|item| item.text == "/ping" && item.role == SandboxSpeakerRole::User)
        );

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
        service
            .write(
                "tester",
                SandboxWriteRequest {
                    operation_id: "op-live".into(),
                    expected_revision: after_observe.revision,
                    action: SandboxAction::SetMode {
                        mode: SandboxMode::Live,
                    },
                },
            )
            .await
            .unwrap();
        let live = service.snapshot("").await.unwrap();
        assert_eq!(live.mode, SandboxMode::Live);
        assert_eq!(live.conversations[0].users[0].display_name, "群友甲");
        service
            .write(
                "tester",
                SandboxWriteRequest {
                    operation_id: "op-send".into(),
                    expected_revision: live.revision,
                    action: SandboxAction::SendAsBot {
                        conversation_id: live.conversations[0].conversation_id.clone(),
                        text: "后台回复".into(),
                    },
                },
            )
            .await
            .unwrap();
        assert_eq!(
            runtime.deliver.lock().expect("deliver").as_slice(),
            ["后台回复"]
        );

        let error = service
            .write(
                "tester",
                SandboxWriteRequest {
                    operation_id: "op-forge".into(),
                    expected_revision: live.revision + 1,
                    action: SandboxAction::IngestAsUser {
                        conversation_id: "missing".into(),
                        user_id: "alice".into(),
                        text: "伪造".into(),
                        inject_into_flow: false,
                    },
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid_state");
    }
}
