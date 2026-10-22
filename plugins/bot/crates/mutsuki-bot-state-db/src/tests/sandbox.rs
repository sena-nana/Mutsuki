//! Sandbox history persistence, migrations and backfills on the shared actor connection.

use super::*;

#[tokio::test]
async fn sandbox_history_roundtrip_survives_reopen() {
    use mutsuki_bot_sandbox::{
        SandboxAction, SandboxApi, SandboxHistoryKind, SandboxHistoryStore, SandboxMode,
        SandboxService, sandbox_user_id,
    };

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.db");
    let repository = Arc::new(BotStateDbRepository::open(&path).unwrap());
    let snapshot = repository.inspect_snapshot().unwrap();
    assert!(
        snapshot
            .tables
            .iter()
            .any(|table| table.name == "bot_sandbox_conversation")
    );
    assert!(
        snapshot
            .tables
            .iter()
            .any(|table| table.name == "bot_sandbox_sticker")
    );
    assert!(
        snapshot
            .tables
            .iter()
            .any(|table| table.name == "bot_sandbox_face")
    );

    let first = SandboxService::with_history("qq-main", repository.clone()).unwrap();
    first.set_runtime(Arc::new(NoopSandboxRuntime));
    let initial = first.snapshot("").await.unwrap();
    let group_id = initial
        .conversations
        .iter()
        .find(|item| item.kind == BotConversationKind::Group)
        .expect("seed group")
        .conversation_id
        .clone();
    first
        .write(
            "tester",
            mutsuki_bot_sandbox::SandboxWriteRequest {
                operation_id: "op-hist".into(),
                expected_revision: initial.revision,
                action: SandboxAction::IngestAsUser {
                    conversation_id: group_id.clone(),
                    user_id: sandbox_user_id("Alice"),
                    text: "persisted".into(),
                    segments: vec![],
                    reply_to: None,
                },
            },
        )
        .await
        .unwrap();
    first
        .upload_sticker("pack.png", "image/png", b"sticker-bytes".to_vec())
        .await
        .unwrap();
    let after = first.snapshot("").await.unwrap();
    first
        .write(
            "tester",
            mutsuki_bot_sandbox::SandboxWriteRequest {
                operation_id: "op-face".into(),
                expected_revision: after.revision,
                action: SandboxAction::IngestAsUser {
                    conversation_id: group_id.clone(),
                    user_id: sandbox_user_id("Alice"),
                    text: String::new(),
                    segments: vec![MessageSegment::PlatformSpecific {
                        platform: "qqbot".into(),
                        kind: "face".into(),
                        payload: serde_json::json!({ "face_type": "6", "face_id": "0" }),
                    }],
                    reply_to: None,
                },
            },
        )
        .await
        .unwrap();
    drop(first);
    drop(repository);

    let reopened = Arc::new(BotStateDbRepository::open(&path).unwrap());
    let restored = SandboxService::with_history("qq-main", reopened.clone()).unwrap();
    let messages = restored.messages(&group_id).await.unwrap();
    assert!(messages.iter().any(|item| item.text == "persisted"));
    assert_eq!(
        restored.snapshot("").await.unwrap().mode,
        SandboxMode::Simulate
    );
    let loaded = SandboxHistoryStore::load(reopened.as_ref()).unwrap();
    assert!(loaded.simulate.iter().any(|item| {
        item.messages
            .iter()
            .any(|message| message.text == "persisted")
    }));
    assert!(
        reopened
            .sandbox_conversations(SandboxHistoryKind::Simulate)
            .unwrap()
            .iter()
            .any(|item| item.conversation_id == group_id)
    );
    assert!(
        reopened
            .sandbox_messages(SandboxHistoryKind::Simulate, &group_id)
            .unwrap()
            .iter()
            .any(|item| item.text == "persisted")
    );
    assert!(loaded.stickers.iter().any(|item| item.name == "pack.png"));
    let sticker_id = loaded
        .stickers
        .iter()
        .find(|item| item.name == "pack.png")
        .expect("sticker")
        .content_hash
        .clone();
    assert_eq!(
        restored.sticker_blob(&sticker_id).await.unwrap().bytes,
        b"sticker-bytes"
    );
    assert!(
        loaded
            .faces
            .iter()
            .any(|item| item.face_key == "qq:6:0" && item.face_type == "6")
    );
}

#[tokio::test]
async fn sandbox_conversation_digest_changes_for_anything_persist_writes() {
    use mutsuki_bot_sandbox::{
        SandboxAction, SandboxApi, SandboxHistoryStore, SandboxService, sandbox_user_id,
    };

    let root = tempfile::tempdir().unwrap();
    let repository = Arc::new(BotStateDbRepository::open(root.path().join("state.db")).unwrap());
    let service = SandboxService::with_history("qq-main", repository.clone()).unwrap();
    service.set_runtime(Arc::new(NoopSandboxRuntime));
    let initial = service.snapshot("").await.unwrap();
    let group_id = initial
        .conversations
        .iter()
        .find(|item| item.kind == BotConversationKind::Group)
        .expect("seed group")
        .conversation_id
        .clone();
    service
        .write(
            "tester",
            mutsuki_bot_sandbox::SandboxWriteRequest {
                operation_id: "op-digest".into(),
                expected_revision: initial.revision,
                action: SandboxAction::IngestAsUser {
                    conversation_id: group_id.clone(),
                    user_id: sandbox_user_id("Alice"),
                    text: "first".into(),
                    segments: vec![],
                    reply_to: None,
                },
            },
        )
        .await
        .unwrap();

    let loaded = SandboxHistoryStore::load(repository.as_ref()).unwrap();
    let conversation = loaded
        .simulate
        .iter()
        .find(|item| item.view.conversation_id == group_id)
        .expect("persisted conversation");
    let baseline = sandbox_history::conversation_digest(conversation).unwrap();

    // Skipping an unchanged conversation is only safe if the digest is stable for
    // identical content...
    assert_eq!(
        baseline,
        sandbox_history::conversation_digest(&conversation.clone()).unwrap()
    );

    // ...and differs for every part the upserts write. A collision here would
    // silently drop a real change from the database.
    let mut retitled = conversation.clone();
    retitled.view.title = format!("{}-renamed", retitled.view.title);
    assert_ne!(
        baseline,
        sandbox_history::conversation_digest(&retitled).unwrap(),
        "conversation row change must not be skipped"
    );

    let mut reactivated = conversation.clone();
    reactivated.view.active_message = !reactivated.view.active_message;
    assert_ne!(
        baseline,
        sandbox_history::conversation_digest(&reactivated).unwrap(),
        "active_message change must not be skipped"
    );

    let mut renamed_user = conversation.clone();
    let user = renamed_user.users.first_mut().expect("seed user");
    user.display_name = format!("{}-renamed", user.display_name);
    assert_ne!(
        baseline,
        sandbox_history::conversation_digest(&renamed_user).unwrap(),
        "user row change must not be skipped"
    );

    let mut edited_message = conversation.clone();
    let message = edited_message.messages.first_mut().expect("seed message");
    message.text = format!("{}-edited", message.text);
    assert_ne!(
        baseline,
        sandbox_history::conversation_digest(&edited_message).unwrap(),
        "message row change must not be skipped"
    );

    let mut appended = conversation.clone();
    let mut extra = appended.messages.last().expect("seed message").clone();
    extra.message_id = format!("{}-extra", extra.message_id);
    appended.messages.push(extra);
    assert_ne!(
        baseline,
        sandbox_history::conversation_digest(&appended).unwrap(),
        "appended message must not be skipped"
    );
}

#[tokio::test]
async fn sandbox_history_load_keeps_blobs_lazy_and_empty_upsert_does_not_clobber() {
    use mutsuki_bot_sandbox::{SandboxApi, SandboxHistoryStore, SandboxService};

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.db");
    let repository = Arc::new(BotStateDbRepository::open(&path).unwrap());
    let first = SandboxService::with_history("qq-main", repository.clone()).unwrap();
    first.set_runtime(Arc::new(NoopSandboxRuntime));
    let uploaded = first
        .upload_media("pic.png", "image/png", b"image-bytes".to_vec())
        .await
        .unwrap();
    drop(first);
    drop(repository);

    let reopened = Arc::new(BotStateDbRepository::open(&path).unwrap());
    let loaded = SandboxHistoryStore::load(reopened.as_ref()).unwrap();
    let asset = loaded
        .media
        .iter()
        .find(|item| item.content_hash == uploaded.media_id)
        .expect("asset metadata");
    assert!(asset.bytes.is_empty());
    let blob = SandboxHistoryStore::load_media_blob(reopened.as_ref(), &uploaded.media_id)
        .unwrap()
        .expect("blob");
    assert_eq!(blob.bytes, b"image-bytes");

    SandboxHistoryStore::save(reopened.as_ref(), &loaded).unwrap();
    let blob_after_empty_upsert =
        SandboxHistoryStore::load_media_blob(reopened.as_ref(), &uploaded.media_id)
            .unwrap()
            .expect("blob after empty upsert");
    assert_eq!(blob_after_empty_upsert.bytes, b"image-bytes");
}

#[test]
fn sandbox_history_migrates_segments_json_to_hash_refs() {
    use mutsuki_bot_sandbox::SandboxHistoryStore;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("legacy.db");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        let conversation = serde_json::json!({
            "version": 1,
            "account_id": "qq-main",
            "kind": "group",
            "group_id": "sandbox:default"
        })
        .to_string();
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE bot_sandbox_meta(singleton INTEGER PRIMARY KEY CHECK(singleton = 1), mode TEXT NOT NULL, account_id TEXT NOT NULL);
                 CREATE TABLE bot_sandbox_conversation(
                     store TEXT NOT NULL, conversation_id TEXT NOT NULL, account_id TEXT NOT NULL, kind TEXT NOT NULL,
                     title TEXT NOT NULL, avatar_url TEXT, conversation_json TEXT NOT NULL, last_preview TEXT,
                     last_activity_unix_ms INTEGER NOT NULL, message_count INTEGER NOT NULL, active_message INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY(store, conversation_id));
                 CREATE TABLE bot_sandbox_message(
                     store TEXT NOT NULL, message_id TEXT NOT NULL, conversation_id TEXT NOT NULL, sender_id TEXT NOT NULL,
                     sender_name TEXT NOT NULL, role TEXT NOT NULL, text TEXT NOT NULL, segments_json TEXT NOT NULL,
                     reply_to TEXT, time_ms INTEGER NOT NULL, PRIMARY KEY(store, message_id));
                 CREATE TABLE bot_sandbox_media(
                     media_id TEXT PRIMARY KEY, mime TEXT NOT NULL, name TEXT NOT NULL, bytes BLOB NOT NULL,
                     created_at_unix_ms INTEGER NOT NULL);
                 INSERT INTO bot_sandbox_meta VALUES (1, 'simulate', 'qq-main');
                 INSERT INTO bot_sandbox_media VALUES ('media-old', 'image/png', 'pic.png', x'706e67', 1);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO bot_sandbox_conversation VALUES ('simulate', 'sandbox:default', 'qq-main', 'group', '群', NULL, ?1, 'pic', 1, 1, 0)",
                rusqlite::params![conversation],
            )
            .unwrap();
        let segments = serde_json::json!([{
            "type": "platform_specific",
            "platform": "sandbox",
            "kind": "media",
            "payload": {"media_id": "media-old", "mime": "image/png", "name": "pic.png"}
        }]);
        connection
            .execute(
                "INSERT INTO bot_sandbox_message VALUES ('simulate', 'msg-1', 'sandbox:default', 'u', 'Alice', 'user', '[图片]', ?1, NULL, 1)",
                rusqlite::params![segments.to_string()],
            )
            .unwrap();
    }

    let repository = BotStateDbRepository::open(&path).unwrap();
    let loaded = SandboxHistoryStore::load(&repository).unwrap();
    let hash = mutsuki_bot_sandbox::hash_bytes(b"png");
    assert_eq!(loaded.media.len(), 1);
    assert_eq!(loaded.media[0].content_hash, hash);
    let message = loaded.simulate[0]
        .messages
        .iter()
        .find(|item| item.message_id == "msg-1")
        .unwrap();
    assert_eq!(message.refs[0].h.as_deref(), Some(hash.as_str()));
    assert!(repository.sandbox_media(&hash).unwrap().is_some());
}

#[test]
fn sandbox_history_backfills_official_face_ids() {
    use mutsuki_bot_sandbox::SandboxHistoryStore;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("faces.db");
    {
        let _repository = BotStateDbRepository::open(&path).unwrap();
    }
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        let conversation = serde_json::json!({
            "version": 1,
            "platform": "qqbot",
            "account_id": "qq-main",
            "kind": "group",
            "group_id": "sandbox:default"
        })
        .to_string();
        connection
            .execute(
                "INSERT INTO bot_sandbox_conversation(
                         store, conversation_id, account_id, kind, title, avatar_url, conversation_json,
                         last_preview, last_activity_unix_ms, message_count, active_message
                     ) VALUES ('simulate', 'sandbox:default', 'qq-main', 'group', '群', NULL, ?1, '', 1, 1, 0)",
                rusqlite::params![conversation],
            )
            .unwrap();
        let refs = serde_json::json!([{ "t": "emoji", "at": 0, "id": "qq:6:0" }]);
        connection
            .execute(
                "INSERT INTO bot_sandbox_message(
                         store, message_id, conversation_id, sender_id, sender_name, role, text, refs_json, reply_to, time_ms
                     ) VALUES ('simulate', 'msg-face', 'sandbox:default', 'u', 'Alice', 'user', '', ?1, NULL, 9)",
                rusqlite::params![refs.to_string()],
            )
            .unwrap();
    }
    let repository = BotStateDbRepository::open(&path).unwrap();
    let loaded = SandboxHistoryStore::load(&repository).unwrap();
    assert!(
        loaded
            .faces
            .iter()
            .any(|item| item.face_key == "qq:6:0" && item.face_id == "0")
    );
}

#[test]
fn sandbox_history_backfills_live_active_message_once() {
    use mutsuki_bot_sandbox::SandboxHistoryStore;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("active-message.db");
    let conversation = serde_json::json!({
        "version": 1,
        "account_id": "qq-main",
        "kind": "group",
        "group_id": "group-1"
    })
    .to_string();
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA user_version=9;
                 CREATE TABLE bot_sandbox_meta(singleton INTEGER PRIMARY KEY CHECK(singleton = 1), mode TEXT NOT NULL, account_id TEXT NOT NULL);
                 CREATE TABLE bot_sandbox_conversation(
                     store TEXT NOT NULL, conversation_id TEXT NOT NULL, account_id TEXT NOT NULL, kind TEXT NOT NULL,
                     title TEXT NOT NULL, avatar_url TEXT, conversation_json TEXT NOT NULL, last_preview TEXT,
                     last_activity_unix_ms INTEGER NOT NULL, message_count INTEGER NOT NULL, active_message INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY(store, conversation_id));
                 INSERT INTO bot_sandbox_meta VALUES (1, 'live', 'qq-main');",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO bot_sandbox_conversation VALUES ('live', 'live-group', 'qq-main', 'group', '群', NULL, ?1, '', 1, 0, 0)",
                rusqlite::params![conversation],
            )
            .unwrap();
    }
    let loaded = {
        let repository = BotStateDbRepository::open(&path).unwrap();
        SandboxHistoryStore::load(&repository).unwrap()
    };
    assert!(loaded.live[0].view.active_message);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE bot_sandbox_conversation SET active_message=0 WHERE store='live'",
            [],
        )
        .unwrap();
    let loaded = {
        let repository = BotStateDbRepository::open(&path).unwrap();
        SandboxHistoryStore::load(&repository).unwrap()
    };
    assert!(!loaded.live[0].view.active_message);
}

struct NoopSandboxRuntime;

#[async_trait]
impl mutsuki_bot_sandbox::SandboxRuntime for NoopSandboxRuntime {
    fn live_available(&self) -> bool {
        true
    }

    async fn ingest(
        &self,
        _event: mutsuki_bot_protocol::BotEvent,
    ) -> Result<(), mutsuki_bot_sandbox::SandboxError> {
        Ok(())
    }

    async fn deliver(
        &self,
        _operation_id: &str,
        _conversation: &BotConversationRef,
        _segments: &[MessageSegment],
        _reply_to: Option<&str>,
    ) -> Result<serde_json::Value, mutsuki_bot_sandbox::SandboxError> {
        Ok(serde_json::json!({ "delivered": true }))
    }
}
