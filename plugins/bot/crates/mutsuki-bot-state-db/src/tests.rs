use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mutsuki_bot_conversation::{
    AgentEventClaim, ConversationContextStore, ConversationRepository, PersonaStore,
};
use mutsuki_bot_delivery::{DeliveryError, DeliveryRepository, ReplyDeliveryRepository};
use mutsuki_bot_interaction::InteractionRepository;
use mutsuki_bot_protocol::{
    AgentSessionBinding, BOT_CONVERSATION_REF_VERSION, BotActiveDeliveryRequest,
    BotConversationKind, BotConversationRef, BotDeliveryAttempt, BotDeliveryContent,
    BotDeliveryReceipt, BotInteractionSession, BotPersona, BotReplyDeliveryPart,
    BotReplyDeliveryRequest, ConversationIclEntry, DeliveryPolicy, DeliveryStatus,
    InteractionScope, InteractionStatus, InteractionWaitSpec, MessageSegment,
};
use rusqlite::Connection;
use tokio::task::JoinSet;

use super::*;
use crate::delivery::CLAIM_DUE_BATCH_LIMIT;
use crate::repository::BUSY_TIMEOUT;

mod sandbox;

#[test]
fn delivery_receipt_status_column_and_index_migrate_from_legacy_schema() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("legacy.db");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                     CREATE TABLE bot_delivery_request(
                         delivery_id TEXT PRIMARY KEY,
                         idempotency_key TEXT NOT NULL UNIQUE,
                         body TEXT NOT NULL
                     );
                     CREATE TABLE bot_delivery_receipt(
                         delivery_id TEXT PRIMARY KEY,
                         idempotency_key TEXT NOT NULL UNIQUE,
                         body TEXT NOT NULL,
                         FOREIGN KEY(delivery_id) REFERENCES bot_delivery_request(delivery_id)
                     );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO bot_delivery_request(delivery_id, idempotency_key, body)
                     VALUES (?1, ?2, ?3)",
                rusqlite::params!["d1", "k1", "{}"],
            )
            .unwrap();
        let body = serde_json::to_string(&BotDeliveryReceipt {
            delivery_id: "d1".into(),
            idempotency_key: "k1".into(),
            status: DeliveryStatus::RetryScheduled,
            attempt_count: 1,
            platform_message_ids: vec![],
            part_receipts: vec![],
            delivered_at_unix_ms: None,
            error_code: None,
            generation: 0,
            lease_expires_at_unix_ms: None,
        })
        .unwrap();
        connection
            .execute(
                "INSERT INTO bot_delivery_receipt(delivery_id, idempotency_key, body)
                     VALUES (?1, ?2, ?3)",
                rusqlite::params!["d1", "k1", body],
            )
            .unwrap();
    }

    let _repository = BotStateDbRepository::open(&path).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    let columns = connection
        .prepare("PRAGMA table_info(bot_delivery_receipt)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(columns.iter().any(|column| column == "status"));
    let status: String = connection
        .query_row(
            "SELECT status FROM bot_delivery_receipt WHERE delivery_id='d1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "retry_scheduled");
    let index_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='index' AND name='bot_delivery_receipt_status'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(index_sql.contains("status"));
    assert!(index_sql.contains("delivery_id"));
}

#[test]
fn unused_conversation_policy_tables_are_dropped_on_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("policy-legacy.db");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA user_version=12;
                     CREATE TABLE bot_conversation_policy(
                         rule_id TEXT PRIMARY KEY,
                         body TEXT NOT NULL
                     );
                     CREATE TABLE bot_conversation_policy_meta(
                         singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                         revision INTEGER NOT NULL
                     );
                     INSERT INTO bot_conversation_policy_meta(singleton, revision) VALUES (1, 0);
                     CREATE TABLE bot_conversation_policy_audit(
                         revision INTEGER PRIMARY KEY,
                         audit_id TEXT NOT NULL UNIQUE,
                         body TEXT NOT NULL
                     );
                     INSERT INTO bot_conversation_policy(rule_id, body) VALUES ('legacy', '{}');",
            )
            .unwrap();
    }

    let repository = BotStateDbRepository::open(&path).unwrap();
    assert_conversation_policy_tables_absent(&repository.inspect_snapshot().unwrap());
    drop(repository);

    let reopened = BotStateDbRepository::open(&path).unwrap();
    assert_conversation_policy_tables_absent(&reopened.inspect_snapshot().unwrap());
    drop(reopened);

    let connection = rusqlite::Connection::open(&path).unwrap();
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(user_version, 14);
    let leftover: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name IN (
                     'bot_conversation_policy',
                     'bot_conversation_policy_meta',
                     'bot_conversation_policy_audit'
                 )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(leftover, 0);

    let fresh = root.path().join("policy-fresh.db");
    let repository = BotStateDbRepository::open(&fresh).unwrap();
    drop(repository);
    let reopened = BotStateDbRepository::open(&fresh).unwrap();
    assert_conversation_policy_tables_absent(&reopened.inspect_snapshot().unwrap());
}

#[test]
fn load_icl_keeps_the_newest_entries_in_chronological_order() {
    let root = tempfile::tempdir().unwrap();
    let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
    // Retention is applied on write with a generous cap so the read side is what
    // this exercises.
    for index in 0..6_u64 {
        repository
            .record_icl(
                "group:g1",
                ConversationIclEntry {
                    actor_id: "u1".into(),
                    display_name: None,
                    text: format!("m{index}"),
                    time_ms: index.cast_signed(),
                },
                100,
            )
            .unwrap();
    }

    // The window is the *newest* entries, still oldest-first: an ICL prompt built
    // from a reversed window would feed the model the conversation backwards.
    let window = repository.load_icl("group:g1", 3).unwrap();
    let texts: Vec<_> = window.iter().map(|entry| entry.text.as_str()).collect();
    assert_eq!(texts, vec!["m3", "m4", "m5"]);

    // 0 means "no limit" and must not be turned into an empty result.
    let all = repository.load_icl("group:g1", 0).unwrap();
    let all_texts: Vec<_> = all.iter().map(|entry| entry.text.as_str()).collect();
    assert_eq!(all_texts, vec!["m0", "m1", "m2", "m3", "m4", "m5"]);

    // A window wider than the history returns everything, unpadded.
    assert_eq!(repository.load_icl("group:g1", 50).unwrap().len(), 6);
}

#[test]
fn icl_and_persona_persist_on_shared_repository() {
    let root = tempfile::tempdir().unwrap();
    let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
    repository
        .record_icl(
            "group:g1",
            ConversationIclEntry {
                actor_id: "u1".into(),
                display_name: Some("Alice".into()),
                text: "hello".into(),
                time_ms: 1,
            },
            20,
        )
        .unwrap();
    let entries = repository.load_icl("group:g1", 20).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].text, "hello");

    repository
        .upsert(BotPersona {
            persona_id: "guide".into(),
            name: "向导".into(),
            system_prompt: "hi".into(),
            begin_dialogs: Vec::new(),
            allowed_tools: None,
            agent_runtime_profile_id: "qq-guide".into(),
        })
        .unwrap();
    repository.bind_conversation("group:g1", "guide").unwrap();
    assert_eq!(
        repository
            .conversation_persona("group:g1")
            .unwrap()
            .as_deref(),
        Some("guide")
    );
}

#[tokio::test]
async fn claim_due_drains_beyond_batch_limit_without_starving_on_sending() {
    let root = tempfile::tempdir().unwrap();
    let repository = Arc::new(BotStateDbRepository::open(root.path().join("state.db")).unwrap());
    let conversation = conversation();
    let count = 80_usize;
    for index in 0..count {
        let request = delivery(
            &conversation,
            &format!("batch-{index:03}"),
            &format!("batch-key-{index:03}"),
        );
        assert!(repository.reserve(&request).await.unwrap().is_none());
    }

    let mut claimed = Vec::new();
    loop {
        let batch = repository.claim_due_delivery_ids(10).await.unwrap();
        if batch.is_empty() {
            break;
        }
        assert!(batch.len() <= usize::try_from(CLAIM_DUE_BATCH_LIMIT).unwrap());
        claimed.extend(batch);
    }
    assert_eq!(claimed.len(), count);
    assert_eq!(claimed[0], "batch-000");
}

#[tokio::test]
async fn pending_after_reserve_and_expired_sending_are_recoverable() {
    let root = tempfile::tempdir().unwrap();
    let repository = Arc::new(BotStateDbRepository::open(root.path().join("state.db")).unwrap());
    let conversation = conversation();
    let request = delivery(&conversation, "crash-pending", "crash-key");
    assert!(repository.reserve(&request).await.unwrap().is_none());

    assert_eq!(
        repository.claim_due_delivery_ids(10).await.unwrap(),
        vec!["crash-pending"]
    );
    let sending = repository.receipt("crash-pending").await.unwrap();
    assert_eq!(sending.status, DeliveryStatus::Sending);
    assert!(sending.lease_expires_at_unix_ms.is_some());

    let mut expired = sending.clone();
    expired.lease_expires_at_unix_ms = Some(5);
    repository.save_receipt(expired).await.unwrap();
    assert!(
        repository
            .claim_due_delivery_ids(10)
            .await
            .unwrap()
            .is_empty()
    );
    let reconciled = repository.receipt("crash-pending").await.unwrap();
    assert_eq!(reconciled.status, DeliveryStatus::ReconcileRequired);

    let retry_request = delivery(&conversation, "race", "race-key");
    assert!(repository.reserve(&retry_request).await.unwrap().is_none());
    repository
        .save_outcome(retry_attempt("race"), retry_receipt("race", "race-key"))
        .await
        .unwrap();
    let mut claims = JoinSet::new();
    for _ in 0..32 {
        let repository = repository.clone();
        claims.spawn(async move { repository.claim_due_delivery_ids(150).await.unwrap() });
    }
    let mut winners = 0;
    while let Some(result) = claims.join_next().await {
        if result.unwrap() == vec!["race".to_owned()] {
            winners += 1;
        }
    }
    assert_eq!(winners, 1);
}

#[tokio::test]
async fn reply_bundle_reservation_is_atomic_idempotent_and_claims_one_part_at_a_time() {
    let root = tempfile::tempdir().unwrap();
    let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
    let conversation = conversation();
    let request = reply_delivery(&conversation, "reply", &["reply:part:2", "reply:part:10"]);

    assert!(repository.reserve_reply(&request).await.unwrap().is_none());
    let duplicate = repository.reserve_reply(&request).await.unwrap().unwrap();
    assert_eq!(duplicate.part_receipts.len(), 2);
    assert!(
        duplicate
            .part_receipts
            .iter()
            .all(|receipt| receipt.status == DeliveryStatus::Pending)
    );
    assert!(
        repository
            .claim_due_delivery_ids(10)
            .await
            .unwrap()
            .is_empty()
    );

    assert_eq!(
        repository.claim_due_reply_part_id(10).await.unwrap(),
        Some("reply:part:2".into())
    );
    assert_eq!(
        repository.receipt("reply:part:2").await.unwrap().status,
        DeliveryStatus::Sending
    );
    assert_eq!(
        repository.receipt("reply:part:10").await.unwrap().status,
        DeliveryStatus::Pending
    );
    assert!(
        repository
            .claim_due_reply_part_id(10)
            .await
            .unwrap()
            .is_none()
    );
    let mut first_succeeded = repository.receipt("reply:part:2").await.unwrap();
    first_succeeded.status = DeliveryStatus::Succeeded;
    first_succeeded.lease_expires_at_unix_ms = None;
    repository.save_receipt(first_succeeded).await.unwrap();
    assert_eq!(
        repository.claim_due_reply_part_id(10).await.unwrap(),
        Some("reply:part:10".into())
    );

    let occupied = delivery(&conversation, "occupied", "occupied-key");
    assert!(repository.reserve(&occupied).await.unwrap().is_none());
    let conflicting = reply_delivery(
        &conversation,
        "conflicting-reply",
        &["new-part", "occupied"],
    );
    assert_eq!(
        repository.reserve_reply(&conflicting).await.unwrap_err(),
        DeliveryError::Conflict
    );
    assert_eq!(
        repository
            .reply_receipt("conflicting-reply")
            .await
            .unwrap_err(),
        DeliveryError::NotFound
    );
    assert_eq!(
        repository.request("new-part").await.unwrap_err(),
        DeliveryError::NotFound
    );
}

#[tokio::test]
async fn unsent_reply_bundle_can_replace_parts_until_a_part_succeeds() {
    let root = tempfile::tempdir().unwrap();
    let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
    let conversation = conversation();
    let request = reply_delivery(&conversation, "replace-reply", &["replace:part:0"]);
    assert!(repository.reserve_reply(&request).await.unwrap().is_none());

    let mut upgraded = request.clone();
    upgraded.parts[0].content.reply_to = Some("quoted-message".into());
    upgraded.parts.push(BotReplyDeliveryPart {
        part_id: "replace:part:1".into(),
        content: BotDeliveryContent {
            segments: vec![MessageSegment::text("second")],
            summary: None,
            reply_to: Some("quoted-message".into()),
        },
        not_before_unix_ms: None,
    });
    assert!(repository.reserve_reply(&upgraded).await.unwrap().is_none());
    let receipt = repository.reply_receipt("replace-reply").await.unwrap();
    assert_eq!(receipt.part_receipts.len(), 2);
    assert!(
        receipt
            .part_receipts
            .iter()
            .all(|part| part.status == DeliveryStatus::Pending)
    );
    assert_eq!(
        repository
            .request("replace:part:1")
            .await
            .unwrap()
            .content
            .reply_to
            .as_deref(),
        Some("quoted-message")
    );

    let mut first = repository.receipt("replace:part:0").await.unwrap();
    first.status = DeliveryStatus::Succeeded;
    first.lease_expires_at_unix_ms = None;
    repository.save_receipt(first).await.unwrap();
    let mut after_send = upgraded.clone();
    after_send.parts[0].content.reply_to = Some("rewrite-succeeded".into());
    assert_eq!(
        repository.reserve_reply(&after_send).await.unwrap_err(),
        DeliveryError::Conflict
    );
    assert_eq!(
        repository
            .request("replace:part:0")
            .await
            .unwrap()
            .content
            .reply_to
            .as_deref(),
        Some("quoted-message")
    );
}

#[tokio::test]
async fn occupancy_only_reply_is_not_due_until_submit_clears_hold() {
    let root = tempfile::tempdir().unwrap();
    let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
    let conversation = conversation();
    let mut draft = reply_delivery(&conversation, "hold-reply", &["hold:part:0"]);
    draft.occupancy_only = true;
    assert!(repository.reserve_reply(&draft).await.unwrap().is_none());
    assert!(
        repository
            .claim_due_reply_part_id(10)
            .await
            .unwrap()
            .is_none()
    );

    let mut finalized = draft.clone();
    finalized.occupancy_only = false;
    finalized.parts[0].content.reply_to = Some("quoted".into());
    assert!(
        repository
            .reserve_reply(&finalized)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        repository.claim_due_reply_part_id(10).await.unwrap(),
        Some("hold:part:0".into())
    );
    assert_eq!(
        repository
            .request("hold:part:0")
            .await
            .unwrap()
            .content
            .reply_to
            .as_deref(),
        Some("quoted")
    );
}

#[tokio::test]
async fn state_recovers_after_reopen_and_uses_one_actor_connection() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("bot-state.db");
    let repository = BotStateDbRepository::open(&path).unwrap();
    let conversation = conversation();
    repository
        .compare_and_set_session_binding(
            &conversation.origin_key(),
            None,
            binding(&conversation, 1),
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .begin_agent_event(&conversation.origin_key(), "event", "turn")
            .await
            .unwrap(),
        AgentEventClaim::New
    );
    repository
        .complete_agent_event(&conversation.origin_key(), "event")
        .await
        .unwrap();
    let request = delivery(&conversation, "delivery", "key");
    assert!(repository.reserve(&request).await.unwrap().is_none());
    repository
        .save_outcome(retry_attempt("delivery"), retry_receipt("delivery", "key"))
        .await
        .unwrap();
    repository.create(interaction(&conversation)).unwrap();
    assert_eq!(repository.metrics().connection_open_count, 1);
    assert_eq!(repository.journal_mode(), "wal");
    drop(repository);

    let reopened = BotStateDbRepository::open(&path).unwrap();
    assert_eq!(reopened.journal_mode(), "wal");
    assert_eq!(
        reopened
            .session_binding(&conversation.origin_key())
            .await
            .unwrap()
            .unwrap()
            .session_id,
        "session"
    );
    assert_eq!(
        reopened
            .begin_agent_event(&conversation.origin_key(), "event", "turn")
            .await
            .unwrap(),
        AgentEventClaim::Completed
    );
    assert_eq!(
        reopened.claim_due_delivery_ids(150).await.unwrap(),
        vec!["delivery"]
    );
    assert_eq!(reopened.recover_waiting().unwrap().len(), 1);
    assert_eq!(reopened.metrics().connection_open_count, 1);
}

#[tokio::test]
async fn management_revision_audit_and_stable_pages_survive_restart() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("management.db");
    let repository = BotStateDbRepository::open(&path).unwrap();
    let conversation = conversation();
    for (delivery_id, key) in [("delivery-b", "key-b"), ("delivery-a", "key-a")] {
        assert!(
            repository
                .reserve(&delivery(&conversation, delivery_id, key))
                .await
                .unwrap()
                .is_none()
        );
    }
    let mut second_interaction = interaction(&conversation);
    second_interaction.session_id = "interaction-b".into();
    repository.create(second_interaction).unwrap();
    let mut first_interaction = interaction(&conversation);
    first_interaction.session_id = "interaction-a".into();
    repository.create(first_interaction).unwrap();

    let deliveries = repository.delivery_page(None, 1).unwrap();
    assert_eq!(deliveries.items[0].0.delivery_id, "delivery-a");
    assert_eq!(deliveries.next_cursor.as_deref(), Some("delivery-a"));
    let interactions = repository.interaction_page(None, 1).unwrap();
    assert_eq!(interactions.items[0].session_id, "interaction-a");
    assert_eq!(interactions.next_cursor.as_deref(), Some("interaction-a"));

    let audit = repository
        .commit_management_audit(
            0,
            "local-web-console",
            "delivery_preview",
            serde_json::json!({"delivery_id": "delivery-a"}),
            100,
        )
        .unwrap()
        .unwrap();
    assert_eq!(audit.revision, 1);
    assert!(
        repository
            .commit_management_audit(0, "forged", "retry", serde_json::Value::Null, 101)
            .unwrap()
            .is_none()
    );
    drop(repository);

    let reopened = BotStateDbRepository::open(path).unwrap();
    assert_eq!(reopened.management_revision().unwrap(), 1);
    let audits = reopened.management_audits(10).unwrap();
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].actor_id, "local-web-console");
    assert_eq!(audits[0].result["delivery_id"], "delivery-a");
}

#[test]
fn management_operation_reservation_is_durable_and_replayable() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("management-operation.db");
    let repository = BotStateDbRepository::open(&path).unwrap();
    assert_eq!(
        repository
            .begin_management_operation("op-1", 0, "console", "send_test", 10)
            .unwrap(),
        BotManagementOperationReservation::Reserved
    );
    assert!(matches!(
        repository.begin_management_operation("op-2", 0, "console", "send_test", 10),
        Err(BotStateDbError::Conflict)
    ));
    drop(repository);

    let reopened = BotStateDbRepository::open(&path).unwrap();
    assert_eq!(
        reopened
            .begin_management_operation("op-1", 0, "console", "send_test", 11)
            .unwrap(),
        BotManagementOperationReservation::Resume
    );
    let audit = reopened
        .complete_management_operation(
            "op-1",
            "send_test",
            serde_json::json!({"message_id": "m1"}),
            12,
        )
        .unwrap();
    assert_eq!(audit.revision, 1);
    assert_eq!(
        reopened
            .begin_management_operation("op-1", 0, "console", "send_test", 13)
            .unwrap(),
        BotManagementOperationReservation::Completed(audit)
    );
    assert!(matches!(
        reopened.begin_management_operation("op-1", 0, "forged", "send_test", 14),
        Err(BotStateDbError::Conflict)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sixty_four_way_reserve_claim_and_cas_are_atomic() {
    let root = tempfile::tempdir().unwrap();
    let repository = Arc::new(BotStateDbRepository::open(root.path().join("state.db")).unwrap());
    let conversation = conversation();

    let mut reservations = JoinSet::new();
    for index in 0_u32..64 {
        let repository = repository.clone();
        let request = delivery(&conversation, &format!("delivery-{index}"), "shared-key");
        reservations.spawn(async move { repository.reserve(&request).await.unwrap() });
    }
    let mut reserved = 0;
    let mut existing = 0;
    while let Some(result) = reservations.join_next().await {
        if result.unwrap().is_none() {
            reserved += 1;
        } else {
            existing += 1;
        }
    }
    assert_eq!((reserved, existing), (1, 63));

    let binding_key = conversation.origin_key();
    repository
        .compare_and_set_session_binding(&binding_key, None, binding(&conversation, 1))
        .await
        .unwrap();
    let mut cas = JoinSet::new();
    for index in 0..64 {
        let repository = repository.clone();
        let binding_key = binding_key.clone();
        let mut next = binding(&conversation, 2);
        next.session_version = index;
        cas.spawn(async move {
            repository
                .compare_and_set_session_binding(&binding_key, Some(1), next)
                .await
        });
    }
    let mut cas_successes = 0;
    while let Some(result) = cas.join_next().await {
        if result.unwrap().is_ok() {
            cas_successes += 1;
        }
    }
    assert_eq!(cas_successes, 1);

    let waiting = interaction(&conversation);
    repository.create(waiting.clone()).unwrap();
    let mut interaction_cas = JoinSet::new();
    for index in 0..64 {
        let repository = repository.clone();
        let mut next = waiting.clone();
        next.version = 2;
        next.retries_remaining = index + 1;
        interaction_cas.spawn(async move { repository.compare_and_set(1, next) });
    }
    let mut interaction_cas_successes = 0;
    while let Some(result) = interaction_cas.join_next().await {
        if result.unwrap().is_ok() {
            interaction_cas_successes += 1;
        }
    }
    assert_eq!(interaction_cas_successes, 1);

    let mut claims = JoinSet::new();
    for _ in 0..64 {
        let repository = repository.clone();
        let binding_key = binding_key.clone();
        claims.spawn(async move {
            repository
                .begin_agent_event(&binding_key, "event-64", "turn")
                .await
                .unwrap()
        });
    }
    let mut claim_counts = BTreeMap::new();
    while let Some(result) = claims.join_next().await {
        *claim_counts
            .entry(format!("{:?}", result.unwrap()))
            .or_insert(0) += 1;
    }
    assert_eq!(claim_counts.get("New"), Some(&1));
    assert_eq!(claim_counts.get("ResumePending"), Some(&63));
    assert_eq!(repository.metrics().connection_open_count, 1);
    assert_eq!(repository.metrics().busy_count, 0);
    assert!(repository.metrics().transaction_count >= 194);
}

#[tokio::test]
async fn failed_delivery_outcome_rolls_back_attempt_and_receipt_together() {
    let root = tempfile::tempdir().unwrap();
    let repository = BotStateDbRepository::open(root.path().join("state.db")).unwrap();
    let conversation = conversation();
    let first = delivery(&conversation, "first", "key-first");
    let second = delivery(&conversation, "second", "key-second");
    assert!(repository.reserve(&first).await.unwrap().is_none());
    assert!(repository.reserve(&second).await.unwrap().is_none());
    repository
        .save_receipt(retry_receipt("first", "key-first"))
        .await
        .unwrap();

    let error = repository
        .save_outcome(
            retry_attempt("second"),
            retry_receipt("second", "key-first"),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, DeliveryError::Repository(_)));
    assert!(repository.attempts("second").await.unwrap().is_empty());
    let second_receipt = repository.receipt("second").await.unwrap();
    assert_eq!(second_receipt.status, DeliveryStatus::Pending);
    assert_eq!(second_receipt.idempotency_key, "key-second");
}

#[tokio::test(flavor = "current_thread")]
async fn database_wait_does_not_block_async_runtime_worker() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.db");
    let repository = BotStateDbRepository::open(&path).unwrap();
    let conversation = conversation();

    let lock_path = path.clone();
    let holder = std::thread::spawn(move || {
        let connection = Connection::open(lock_path).unwrap();
        connection.busy_timeout(Duration::from_millis(1)).unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        std::thread::sleep(Duration::from_millis(250));
        connection.execute_batch("COMMIT").unwrap();
    });
    std::thread::sleep(Duration::from_millis(30));

    let progress = Arc::new(AtomicUsize::new(0));
    let ticker_progress = progress.clone();
    let ticker = async {
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            ticker_progress.fetch_add(1, Ordering::Relaxed);
            tokio::task::yield_now().await;
        }
    };

    let binding_key = conversation.origin_key();
    let (write_result, _) = tokio::join!(
        repository.compare_and_set_session_binding(&binding_key, None, binding(&conversation, 1)),
        ticker
    );
    write_result.unwrap();
    holder.join().unwrap();
    assert!(
        progress.load(Ordering::Relaxed) > 0,
        "async runtime worker must keep scheduling while the DB actor waits on SQLITE_BUSY"
    );
    assert_eq!(repository.metrics().connection_open_count, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_writer_contention_stays_correct() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.db");
    let repository = Arc::new(BotStateDbRepository::open(&path).unwrap());
    let conversation = conversation();
    let binding_key = conversation.origin_key();
    repository
        .compare_and_set_session_binding(&binding_key, None, binding(&conversation, 1))
        .await
        .unwrap();
    let waiting = interaction(&conversation);
    repository.create(waiting.clone()).unwrap();

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lock_path = path.clone();
    let stop_flag = stop.clone();
    let contender = std::thread::spawn(move || {
        let connection = Connection::open(lock_path).unwrap();
        connection.busy_timeout(BUSY_TIMEOUT).unwrap();
        while !stop_flag.load(Ordering::Relaxed) {
            connection.execute_batch("BEGIN IMMEDIATE").unwrap();
            std::thread::sleep(Duration::from_millis(5));
            connection.execute_batch("COMMIT").unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
    });

    let mut jobs = JoinSet::new();
    let iterations = 256_u32;
    for index in 0..iterations {
        let repository = repository.clone();
        let conversation = conversation.clone();
        let binding_key = binding_key.clone();
        let waiting = waiting.clone();
        jobs.spawn(async move {
            let started = Instant::now();
            match index % 4 {
                0 => {
                    let request = delivery(
                        &conversation,
                        &format!("delivery-{index}"),
                        &format!("key-{index}"),
                    );
                    repository.reserve(&request).await.unwrap();
                }
                1 => {
                    let _ = repository
                        .begin_agent_event(&binding_key, &format!("event-{index}"), "turn")
                        .await
                        .unwrap();
                }
                2 => {
                    let mut next = waiting.clone();
                    next.version = u64::from(index) + 2;
                    next.retries_remaining = index;
                    let _ = repository.compare_and_set(1, next);
                }
                _ => {
                    let mut next = binding(&conversation, 2);
                    next.session_version = u64::from(index);
                    let _ = repository
                        .compare_and_set_session_binding(&binding_key, Some(1), next)
                        .await;
                }
            }
            started.elapsed()
        });
    }

    let mut latencies = Vec::with_capacity(iterations as usize);
    while let Some(result) = jobs.join_next().await {
        latencies.push(result.unwrap());
    }
    stop.store(true, Ordering::Relaxed);
    contender.join().unwrap();

    latencies.sort_unstable();
    let p95 = latencies[((latencies.len() * 95) / 100).saturating_sub(1)];
    let p99 = latencies[((latencies.len() * 99) / 100).saturating_sub(1)];
    eprintln!(
        "issue-164 contention: n={} p95={p95:?} p99={p99:?} open={} busy={}",
        latencies.len(),
        repository.metrics().connection_open_count,
        repository.metrics().busy_count
    );

    assert_eq!(repository.metrics().connection_open_count, 1);
    assert_eq!(
        repository.metrics().busy_count,
        0,
        "actor busy_timeout must absorb SQLITE_BUSY instead of surfacing it"
    );
    assert!(
        p95 < Duration::from_secs(2),
        "p95 under injected contention should stay under 2s, got {p95:?}"
    );
    assert!(
        p99 < Duration::from_secs(5),
        "p99 under injected contention should stay under busy_timeout, got {p99:?}"
    );
}

#[test]
fn inspect_reads_live_schema_and_rejects_unknown_tables() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.db");
    let repository = BotStateDbRepository::open(&path).unwrap();
    let snapshot = repository.inspect_snapshot().unwrap();
    assert_eq!(snapshot.path, path.display().to_string());
    assert_eq!(snapshot.journal_mode, "wal");
    assert!(snapshot.size_bytes.unwrap_or(0) > 0);
    let meta = snapshot
        .tables
        .iter()
        .find(|table| table.name == "bot_management_meta")
        .expect("schema table");
    assert_eq!(meta.row_count, 1);
    assert!(meta.columns.iter().any(|column| column.name == "revision"));
    assert!(
        !snapshot
            .tables
            .iter()
            .any(|table| table.name.starts_with("sqlite_"))
    );

    let page = repository
        .inspect_rows("bot_management_meta", None, 10)
        .unwrap();
    assert_eq!(page.table, "bot_management_meta");
    assert!(page.columns.contains(&"revision".to_string()));
    assert_eq!(page.rows.len(), 1);
    assert!(page.next_cursor.is_none());
    assert!(repository.inspect_rows("sqlite_master", None, 10).is_err());
    assert!(repository.inspect_rows("not_a_table", None, 10).is_err());
    assert!(
        repository
            .inspect_rows("bot_management_meta;drop", None, 10)
            .is_err()
    );
}

#[test]
fn blocking_repository_calls_survive_every_runtime_flavor() {
    use mutsuki_bot_sandbox::SandboxHistoryStore;

    let root = tempfile::tempdir().unwrap();
    let repository = Arc::new(BotStateDbRepository::open(root.path().join("flavor.db")).unwrap());

    // `block_in_place` panics outside a multi-threaded runtime, and the sandbox console reaches
    // this repository from both flavors plus plain Runner threads.
    SandboxHistoryStore::load(repository.as_ref()).unwrap();

    let current_thread = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    current_thread.block_on({
        let repository = repository.clone();
        async move { SandboxHistoryStore::load(repository.as_ref()).unwrap() }
    });

    let multi_thread = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    multi_thread.block_on(async move {
        // One worker: the repository wait must release it, otherwise the spawned task never runs.
        let peer = tokio::spawn(async { "alive" });
        SandboxHistoryStore::load(repository.as_ref()).unwrap();
        assert_eq!(peer.await.unwrap(), "alive");
    });
}

fn assert_conversation_policy_tables_absent(snapshot: &BotDatabaseSnapshot) {
    for name in [
        "bot_conversation_policy",
        "bot_conversation_policy_meta",
        "bot_conversation_policy_audit",
    ] {
        assert!(
            !snapshot.tables.iter().any(|table| table.name == name),
            "leftover unused table {name} must not remain"
        );
    }
}

fn conversation() -> BotConversationRef {
    BotConversationRef {
        version: BOT_CONVERSATION_REF_VERSION,
        platform: mutsuki_bot_protocol::BotPlatform::QqBot,
        account_id: "main".into(),
        kind: BotConversationKind::Group,
        user_id: None,
        group_id: Some("group".into()),
        guild_id: None,
        channel_id: None,
        thread_id: None,
    }
}

fn binding(conversation: &BotConversationRef, generation: u64) -> AgentSessionBinding {
    AgentSessionBinding {
        origin_key: conversation.origin_key(),
        session_id: "session".into(),
        session_version: 2,
        last_event_sequence: 3,
        policy_revision: 2,
        generation,
    }
}

fn delivery(
    conversation: &BotConversationRef,
    delivery_id: &str,
    idempotency_key: &str,
) -> BotActiveDeliveryRequest {
    BotActiveDeliveryRequest {
        delivery_id: delivery_id.into(),
        idempotency_key: idempotency_key.into(),
        conversation: conversation.clone(),
        content: BotDeliveryContent {
            segments: vec![MessageSegment::Text {
                text: "hello".into(),
            }],
            summary: None,
            reply_to: None,
        },
        policy: DeliveryPolicy {
            max_attempts: 3,
            initial_backoff_ms: 10,
            max_backoff_ms: 100,
            not_before_unix_ms: None,
            expires_at_unix_ms: None,
        },
        dry_run: false,
        source_execution_id: Some("execution".into()),
    }
}

fn reply_delivery(
    conversation: &BotConversationRef,
    reply_id: &str,
    part_ids: &[&str],
) -> BotReplyDeliveryRequest {
    BotReplyDeliveryRequest {
        reply_id: reply_id.into(),
        idempotency_key: format!("{reply_id}:key"),
        conversation: conversation.clone(),
        parts: part_ids
            .iter()
            .map(|part_id| BotReplyDeliveryPart {
                part_id: (*part_id).into(),
                content: BotDeliveryContent {
                    segments: vec![MessageSegment::text(format!("reply {part_id}"))],
                    summary: None,
                    reply_to: None,
                },
                not_before_unix_ms: None,
            })
            .collect(),
        policy: DeliveryPolicy {
            max_attempts: 3,
            initial_backoff_ms: 10,
            max_backoff_ms: 100,
            not_before_unix_ms: None,
            expires_at_unix_ms: None,
        },
        source_event_id: "event".into(),
        source_turn_id: "turn".into(),
        source_binding_key: None,
        occupancy_only: false,
    }
}

fn retry_attempt(delivery_id: &str) -> BotDeliveryAttempt {
    BotDeliveryAttempt {
        delivery_id: delivery_id.into(),
        attempt: 1,
        status: DeliveryStatus::RetryScheduled,
        started_at_unix_ms: 100,
        retry_at_unix_ms: Some(150),
        error_code: Some("qq.rate_limited".into()),
    }
}

fn retry_receipt(delivery_id: &str, idempotency_key: &str) -> BotDeliveryReceipt {
    BotDeliveryReceipt {
        delivery_id: delivery_id.into(),
        idempotency_key: idempotency_key.into(),
        status: DeliveryStatus::RetryScheduled,
        attempt_count: 1,
        platform_message_ids: Vec::new(),
        part_receipts: Vec::new(),
        delivered_at_unix_ms: None,
        error_code: Some("qq.rate_limited".into()),
        generation: 1,
        lease_expires_at_unix_ms: None,
    }
}

fn interaction(conversation: &BotConversationRef) -> BotInteractionSession {
    BotInteractionSession {
        session_id: "interaction".into(),
        conversation: conversation.clone(),
        scope: InteractionScope::ActorInConversation,
        actor_id: Some("actor".into()),
        state_ref_id: "state-ref".into(),
        wait: InteractionWaitSpec {
            event_kinds: Vec::new(),
            command: Some("verify".into()),
            predicate_service_id: None,
            timeout_at_unix_ms: 1_000,
            retry_prompt: None,
        },
        status: InteractionStatus::Waiting,
        generation: 1,
        version: 1,
        exclusive: true,
        retries_remaining: 2,
    }
}

#[test]
fn v14_migration_makes_stored_conversation_refs_platform_neutral() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("v13.db");
    {
        let _repository = BotStateDbRepository::open(&path).unwrap();
    }
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        let legacy_ref = serde_json::json!({
            "version": 1,
            "account_id": "qq-main",
            "kind": "group",
            "group_id": "group-1"
        });
        let body = serde_json::json!({
            "session_id": "legacy-session",
            "conversation": legacy_ref,
            "account": {"account_id": "qq-main", "platform": "qq_bot"}
        });
        connection.execute_batch("PRAGMA user_version=13;").unwrap();
        connection
            .execute(
                "INSERT INTO bot_interaction(session_id, origin_key, status, version, body)
                 VALUES ('legacy-session', 'qq:v1:legacy', 'closed', 1, ?1)",
                rusqlite::params![body.to_string()],
            )
            .unwrap();
    }
    drop(BotStateDbRepository::open(&path).unwrap());

    let connection = rusqlite::Connection::open(&path).unwrap();
    let body: String = connection
        .query_row(
            "SELECT body FROM bot_interaction WHERE session_id = 'legacy-session'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["conversation"]["platform"], "qqbot");
    assert_eq!(body["account"]["platform"], "qqbot");
    let conversation: mutsuki_bot_protocol::BotConversationRef =
        serde_json::from_value(body["conversation"].clone()).unwrap();
    assert_eq!(
        conversation.platform,
        mutsuki_bot_protocol::BotPlatform::QqBot
    );
    assert!(conversation.origin_key().starts_with("qq:v1:"));
}
