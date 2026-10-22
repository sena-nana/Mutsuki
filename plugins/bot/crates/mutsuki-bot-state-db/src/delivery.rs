//! `DeliveryRepository`: active delivery reservations, attempts, receipts and due claims.

use async_trait::async_trait;
use mutsuki_bot_delivery::{DELIVERY_SEND_LEASE_MS, DeliveryError, DeliveryRepository};
use mutsuki_bot_protocol::{
    BotActiveDeliveryRequest, BotDeliveryAttempt, BotDeliveryReceipt, BotReplyDeliveryRequest,
    DeliveryStatus,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::codec::{
    decode, delivery_status_name, encode, immediate, optional_body, sqlite_integer,
};
use crate::error::{BotStateDbError, delivery_error};
use crate::repository::{BotStateDbRepository, BotStatePage};

#[derive(Debug)]
pub(crate) enum DeliveryReservation {
    Reserved,
    Existing(BotDeliveryReceipt),
    Conflict,
}

fn pending_receipt(request: &BotActiveDeliveryRequest) -> BotDeliveryReceipt {
    BotDeliveryReceipt {
        delivery_id: request.delivery_id.clone(),
        idempotency_key: request.idempotency_key.clone(),
        status: DeliveryStatus::Pending,
        attempt_count: 0,
        platform_message_ids: Vec::new(),
        part_receipts: Vec::new(),
        delivered_at_unix_ms: None,
        error_code: None,
        generation: 0,
        lease_expires_at_unix_ms: None,
    }
}

fn reserve_delivery(
    connection: &mut Connection,
    request: &BotActiveDeliveryRequest,
) -> Result<DeliveryReservation, BotStateDbError> {
    let transaction = immediate(connection)?;
    let reservation = reserve_delivery_in_transaction(&transaction, request)?;
    transaction.commit()?;
    Ok(reservation)
}

pub(crate) fn reserve_delivery_in_transaction(
    transaction: &Transaction<'_>,
    request: &BotActiveDeliveryRequest,
) -> Result<DeliveryReservation, BotStateDbError> {
    let changed = transaction.execute(
        "INSERT OR IGNORE INTO bot_delivery_request(delivery_id, idempotency_key, body)
         VALUES (?1, ?2, ?3)",
        params![
            request.delivery_id,
            request.idempotency_key,
            encode(request)?
        ],
    )?;
    if changed == 1 {
        upsert_delivery_receipt(&transaction, &pending_receipt(request))?;
        return Ok(DeliveryReservation::Reserved);
    }
    if let Some(body) = transaction
        .query_row(
            "SELECT body FROM bot_delivery_receipt WHERE idempotency_key=?1",
            params![request.idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        let receipt = decode(&body)?;
        return Ok(DeliveryReservation::Existing(receipt));
    }
    if let Some(body) = transaction
        .query_row(
            "SELECT body FROM bot_delivery_request WHERE idempotency_key=?1",
            params![request.idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        let owner: BotActiveDeliveryRequest = decode(&body)?;
        let receipt = pending_receipt(&owner);
        upsert_delivery_receipt(&transaction, &receipt)?;
        return Ok(DeliveryReservation::Existing(receipt));
    }
    let delivery_exists = transaction
        .query_row(
            "SELECT 1 FROM bot_delivery_request WHERE delivery_id=?1",
            params![request.delivery_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(if delivery_exists {
        DeliveryReservation::Conflict
    } else {
        return Err(BotStateDbError::Invariant(
            "delivery reservation disappeared after conflict".into(),
        ));
    })
}

fn delivery_request(
    connection: &Connection,
    delivery_id: &str,
) -> Result<Option<BotActiveDeliveryRequest>, BotStateDbError> {
    optional_body(
        connection,
        "SELECT body FROM bot_delivery_request WHERE delivery_id=?1",
        delivery_id,
    )
}

pub(crate) fn delivery_receipt(
    connection: &Connection,
    delivery_id: &str,
) -> Result<Option<BotDeliveryReceipt>, BotStateDbError> {
    optional_body(
        connection,
        "SELECT body FROM bot_delivery_receipt WHERE delivery_id=?1",
        delivery_id,
    )
}

fn delivery_attempts(
    connection: &Connection,
    delivery_id: &str,
) -> Result<Vec<BotDeliveryAttempt>, BotStateDbError> {
    let mut statement = connection.prepare_cached(
        "SELECT body FROM bot_delivery_attempt
         WHERE delivery_id=?1 ORDER BY attempt, status",
    )?;
    statement
        .query_map(params![delivery_id], |row| row.get::<_, String>(0))?
        .map(|body| decode(&body?))
        .collect()
}

pub(crate) fn delivery_page(
    connection: &Connection,
    after: &str,
    limit: u32,
) -> Result<BotStatePage<(BotDeliveryReceipt, Vec<BotDeliveryAttempt>)>, BotStateDbError> {
    let fetch = i64::from(limit.saturating_add(1));
    let mut statement = connection.prepare_cached(
        "SELECT delivery_id FROM bot_delivery_receipt
         WHERE delivery_id > ?1 ORDER BY delivery_id LIMIT ?2",
    )?;
    let mut delivery_ids = statement
        .query_map(params![after, fetch], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let has_more = delivery_ids.len() > limit as usize;
    delivery_ids.truncate(limit as usize);
    let next_cursor = has_more.then(|| delivery_ids.last().cloned()).flatten();
    let items = delivery_ids
        .into_iter()
        .map(|delivery_id| {
            let receipt = delivery_receipt(connection, &delivery_id)?.ok_or_else(|| {
                BotStateDbError::Invariant(format!(
                    "listed delivery receipt disappeared: {delivery_id}"
                ))
            })?;
            let attempts = delivery_attempts(connection, &delivery_id)?;
            Ok((receipt, attempts))
        })
        .collect::<Result<Vec<_>, BotStateDbError>>()?;
    Ok(BotStatePage { items, next_cursor })
}

fn insert_delivery_attempt(
    connection: &Connection,
    attempt: &BotDeliveryAttempt,
) -> Result<usize, BotStateDbError> {
    let attempt_number = i64::from(attempt.attempt);
    let retry_at = attempt.retry_at_unix_ms.map(sqlite_integer).transpose()?;
    Ok(connection.execute(
        "INSERT OR REPLACE INTO bot_delivery_attempt(delivery_id, attempt, status, retry_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            attempt.delivery_id,
            attempt_number,
            delivery_status_name(attempt.status),
            retry_at,
            encode(attempt)?,
        ],
    )?)
}

fn save_delivery_outcome(
    connection: &mut Connection,
    attempt: &BotDeliveryAttempt,
    receipt: &BotDeliveryReceipt,
) -> Result<(), BotStateDbError> {
    let transaction = immediate(connection)?;
    insert_delivery_attempt(&transaction, attempt)?;
    upsert_delivery_receipt(&transaction, receipt)?;
    transaction.commit()?;
    Ok(())
}

fn save_delivery_receipt(
    connection: &Connection,
    receipt: &BotDeliveryReceipt,
) -> Result<(), BotStateDbError> {
    upsert_delivery_receipt(connection, receipt)?;
    Ok(())
}

fn upsert_delivery_receipt(
    connection: &Connection,
    receipt: &BotDeliveryReceipt,
) -> Result<usize, BotStateDbError> {
    Ok(connection.execute(
        "INSERT INTO bot_delivery_receipt(delivery_id, idempotency_key, status, body)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(delivery_id) DO UPDATE SET
             idempotency_key=excluded.idempotency_key,
             status=excluded.status,
             body=excluded.body",
        params![
            receipt.delivery_id,
            receipt.idempotency_key,
            delivery_status_name(receipt.status),
            encode(receipt)?,
        ],
    )?)
}

fn claim_send_lease_on_receipt(receipt: &mut BotDeliveryReceipt, now_unix_ms: u64, lease_ms: u64) {
    receipt.status = DeliveryStatus::Sending;
    receipt.generation = receipt.generation.saturating_add(1);
    receipt.lease_expires_at_unix_ms = Some(now_unix_ms.saturating_add(lease_ms));
    receipt.error_code = None;
}

fn claim_due_deliveries(
    connection: &mut Connection,
    now_unix_ms: u64,
) -> Result<Vec<String>, BotStateDbError> {
    claim_due_deliveries_by_kind(connection, now_unix_ms, false)
}

pub(crate) fn claim_due_reply_parts(
    connection: &mut Connection,
    now_unix_ms: u64,
) -> Result<Vec<String>, BotStateDbError> {
    claim_due_deliveries_by_kind(connection, now_unix_ms, true)
}

pub(crate) const CLAIM_DUE_BATCH_LIMIT: i64 = 64;

fn claim_due_deliveries_by_kind(
    connection: &mut Connection,
    now_unix_ms: u64,
    reply_parts: bool,
) -> Result<Vec<String>, BotStateDbError> {
    let transaction = immediate(connection)?;
    let now = sqlite_integer(now_unix_ms)?;
    let candidates = {
        let query = if reply_parts {
            "SELECT r.delivery_id, r.status, r.body, d.body
             FROM bot_delivery_receipt r
             JOIN bot_reply_delivery_part p ON p.delivery_id=r.delivery_id
             JOIN bot_reply_delivery d ON d.reply_id=p.reply_id
             WHERE r.status IN ('pending', 'retry_scheduled', 'sending')
               AND NOT EXISTS (
                   SELECT 1
                   FROM bot_reply_delivery_part earlier
                   JOIN bot_delivery_receipt earlier_receipt
                     ON earlier_receipt.delivery_id=earlier.delivery_id
                   WHERE earlier.reply_id=p.reply_id
                     AND earlier.part_index<p.part_index
                     AND earlier_receipt.status IN (
                         'pending', 'retry_scheduled', 'sending', 'reconcile_required'
                     )
               )
               AND (
                   r.status = 'pending'
                   OR (
                       r.status = 'retry_scheduled'
                       AND EXISTS (
                           SELECT 1 FROM bot_delivery_attempt a
                           WHERE a.delivery_id=r.delivery_id
                             AND a.status='retry_scheduled'
                             AND a.retry_at<=?1
                             AND a.attempt=(
                                 SELECT MAX(b.attempt) FROM bot_delivery_attempt b
                                 WHERE b.delivery_id=r.delivery_id
                             )
                       )
                   )
                   OR (
                       r.status = 'sending'
                       AND (
                           json_extract(r.body, '$.lease_expires_at_unix_ms') IS NULL
                           OR json_extract(r.body, '$.lease_expires_at_unix_ms') <= ?1
                       )
                   )
               )
             ORDER BY p.reply_id, p.part_index
             LIMIT ?2"
        } else {
            "SELECT r.delivery_id, r.status, r.body, NULL
             FROM bot_delivery_receipt r
             WHERE r.status IN ('pending', 'retry_scheduled', 'sending')
               AND NOT EXISTS (
                   SELECT 1 FROM bot_reply_delivery_part p WHERE p.delivery_id=r.delivery_id
               )
               AND (
                   r.status = 'pending'
                   OR (
                       r.status = 'retry_scheduled'
                       AND EXISTS (
                           SELECT 1 FROM bot_delivery_attempt a
                           WHERE a.delivery_id=r.delivery_id
                             AND a.status='retry_scheduled'
                             AND a.retry_at<=?1
                             AND a.attempt=(
                                 SELECT MAX(b.attempt) FROM bot_delivery_attempt b
                                 WHERE b.delivery_id=r.delivery_id
                             )
                       )
                   )
                   OR (
                       r.status = 'sending'
                       AND (
                           json_extract(r.body, '$.lease_expires_at_unix_ms') IS NULL
                           OR json_extract(r.body, '$.lease_expires_at_unix_ms') <= ?1
                       )
                   )
               )
             ORDER BY r.delivery_id
             LIMIT ?2"
        };
        let mut statement = transaction.prepare_cached(query)?;
        statement
            .query_map(params![now, CLAIM_DUE_BATCH_LIMIT], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut claimed = Vec::new();
    for (delivery_id, status, body, reply_body) in candidates {
        if reply_body
            .as_deref()
            .and_then(|body| decode::<BotReplyDeliveryRequest>(body).ok())
            .is_some_and(|request| request.occupancy_only)
        {
            continue;
        }
        let mut receipt: BotDeliveryReceipt = decode(&body)?;
        match status.as_str() {
            "sending" => {
                let expired = receipt
                    .lease_expires_at_unix_ms
                    .is_none_or(|expires| expires <= now_unix_ms);
                if expired {
                    receipt.status = DeliveryStatus::ReconcileRequired;
                    receipt.lease_expires_at_unix_ms = None;
                    receipt.error_code = Some("delivery.reconcile_required".into());
                    transaction.execute(
                        "UPDATE bot_delivery_receipt SET status='reconcile_required', body=?2
                         WHERE delivery_id=?1 AND status='sending'",
                        params![delivery_id, encode(&receipt)?],
                    )?;
                }
            }
            "pending" => {
                claim_send_lease_on_receipt(&mut receipt, now_unix_ms, DELIVERY_SEND_LEASE_MS);
                let changed = transaction.execute(
                    "UPDATE bot_delivery_receipt SET status='sending', body=?2
                     WHERE delivery_id=?1 AND status='pending'",
                    params![delivery_id, encode(&receipt)?],
                )?;
                if changed == 1 {
                    claimed.push(delivery_id);
                    if reply_parts {
                        break;
                    }
                }
            }
            "retry_scheduled" => {
                // Due-ness was already decided by the `EXISTS` on
                // `bot_delivery_attempt` in the candidate query above, with the same
                // `now` and the same max-attempt predicate. Re-asking per candidate
                // compiled a statement and ran the subquery again up to
                // `CLAIM_DUE_BATCH_LIMIT` times per tick. Nothing in this loop writes
                // `bot_delivery_attempt`, so the answer cannot change underneath us.
                claim_send_lease_on_receipt(&mut receipt, now_unix_ms, DELIVERY_SEND_LEASE_MS);
                let changed = transaction.execute(
                    "UPDATE bot_delivery_receipt SET status='sending', body=?2
                     WHERE delivery_id=?1 AND status='retry_scheduled'",
                    params![delivery_id, encode(&receipt)?],
                )?;
                if changed == 1 {
                    claimed.push(delivery_id);
                    if reply_parts {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    transaction.commit()?;
    Ok(claimed)
}

fn begin_send_delivery(
    connection: &mut Connection,
    delivery_id: &str,
    attempt: BotDeliveryAttempt,
    now_unix_ms: u64,
    lease_ms: u64,
) -> Result<BotDeliveryReceipt, BotStateDbError> {
    let transaction = immediate(connection)?;
    let body = transaction
        .query_row(
            "SELECT body FROM bot_delivery_receipt WHERE delivery_id=?1",
            params![delivery_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            BotStateDbError::Invariant(format!("delivery receipt missing: {delivery_id}"))
        })?;
    let mut receipt: BotDeliveryReceipt = decode(&body)?;
    let claimable = match receipt.status {
        DeliveryStatus::Pending
        | DeliveryStatus::RetryScheduled
        | DeliveryStatus::PermanentlyFailed
        | DeliveryStatus::ReconcileRequired => true,
        DeliveryStatus::Sending => receipt
            .lease_expires_at_unix_ms
            .is_some_and(|expires| expires > now_unix_ms),
        _ => false,
    };
    if !claimable {
        return Err(BotStateDbError::Invariant(format!(
            "delivery {delivery_id} is not claimable in status {:?}",
            receipt.status
        )));
    }
    if !matches!(receipt.status, DeliveryStatus::Sending)
        || receipt
            .lease_expires_at_unix_ms
            .is_none_or(|expires| expires <= now_unix_ms)
    {
        claim_send_lease_on_receipt(&mut receipt, now_unix_ms, lease_ms);
    }
    upsert_delivery_receipt(&transaction, &receipt)?;
    insert_delivery_attempt(&transaction, &attempt)?;
    transaction.commit()?;
    Ok(receipt)
}

#[async_trait]
impl DeliveryRepository for BotStateDbRepository {
    async fn reserve(
        &self,
        request: &BotActiveDeliveryRequest,
    ) -> Result<Option<BotDeliveryReceipt>, DeliveryError> {
        let request = request.clone();
        match self
            .call_sync(true, move |connection| {
                reserve_delivery(connection, &request)
            })
            .map_err(delivery_error)?
        {
            DeliveryReservation::Reserved => Ok(None),
            DeliveryReservation::Existing(receipt) => Ok(Some(receipt)),
            DeliveryReservation::Conflict => Err(DeliveryError::Conflict),
        }
    }

    async fn request(&self, delivery_id: &str) -> Result<BotActiveDeliveryRequest, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(false, move |connection| {
            delivery_request(connection, &delivery_id)
        })
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    async fn receipt(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(false, move |connection| {
            delivery_receipt(connection, &delivery_id)
        })
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    async fn attempts(&self, delivery_id: &str) -> Result<Vec<BotDeliveryAttempt>, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(false, move |connection| {
            delivery_attempts(connection, &delivery_id)
        })
        .map_err(delivery_error)
    }

    /// One actor round trip for the whole claimed batch instead of two per id.
    /// The rows are read on the database thread with the same helpers, so the
    /// missing-row behaviour stays `NotFound` for the batch.
    async fn requests_and_receipts(
        &self,
        delivery_ids: &[String],
    ) -> Result<Vec<(BotActiveDeliveryRequest, BotDeliveryReceipt)>, DeliveryError> {
        let delivery_ids = delivery_ids.to_vec();
        self.call_sync(false, move |connection| {
            let mut rows = Vec::with_capacity(delivery_ids.len());
            for delivery_id in &delivery_ids {
                let (Some(request), Some(receipt)) = (
                    delivery_request(connection, delivery_id)?,
                    delivery_receipt(connection, delivery_id)?,
                ) else {
                    return Ok(None);
                };
                rows.push((request, receipt));
            }
            Ok(Some(rows))
        })
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    /// One actor round trip for every part of a reply bundle.
    async fn receipts(
        &self,
        delivery_ids: &[String],
    ) -> Result<Vec<BotDeliveryReceipt>, DeliveryError> {
        let delivery_ids = delivery_ids.to_vec();
        self.call_sync(false, move |connection| {
            let mut receipts = Vec::with_capacity(delivery_ids.len());
            for delivery_id in &delivery_ids {
                let Some(receipt) = delivery_receipt(connection, delivery_id)? else {
                    return Ok(None);
                };
                receipts.push(receipt);
            }
            Ok(Some(receipts))
        })
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    async fn save_outcome(
        &self,
        attempt: BotDeliveryAttempt,
        receipt: BotDeliveryReceipt,
    ) -> Result<(), DeliveryError> {
        self.call_sync(true, move |connection| {
            save_delivery_outcome(connection, &attempt, &receipt)
        })
        .map_err(delivery_error)
    }

    async fn save_receipt(&self, receipt: BotDeliveryReceipt) -> Result<(), DeliveryError> {
        self.call_sync(false, move |connection| {
            save_delivery_receipt(connection, &receipt)
        })
        .map_err(delivery_error)
    }

    async fn claim_due_delivery_ids(&self, now_unix_ms: u64) -> Result<Vec<String>, DeliveryError> {
        self.call_sync(true, move |connection| {
            claim_due_deliveries(connection, now_unix_ms)
        })
        .map_err(delivery_error)
    }

    async fn begin_send(
        &self,
        delivery_id: &str,
        attempt: BotDeliveryAttempt,
        now_unix_ms: u64,
        lease_ms: u64,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(true, move |connection| {
            begin_send_delivery(connection, &delivery_id, attempt, now_unix_ms, lease_ms)
        })
        .map_err(delivery_error)
    }
}
