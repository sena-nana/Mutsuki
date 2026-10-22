//! `ReplyDeliveryRepository`: multi-part reply bundles layered over delivery reservations.

use async_trait::async_trait;
use mutsuki_bot_delivery::{DeliveryError, ReplyDeliveryRepository, reply_part_request};
use mutsuki_bot_protocol::{BotReplyDeliveryReceipt, BotReplyDeliveryRequest, DeliveryStatus};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::codec::{decode, encode, immediate, optional_body};
use crate::delivery::{
    DeliveryReservation, claim_due_reply_parts, delivery_receipt, reserve_delivery_in_transaction,
};
use crate::error::{BotStateDbError, delivery_error};
use crate::repository::BotStateDbRepository;

#[derive(Debug)]
enum ReplyDeliveryReservation {
    Reserved,
    Existing(BotReplyDeliveryReceipt),
    Conflict,
}

fn reserve_reply_delivery(
    connection: &mut Connection,
    request: &BotReplyDeliveryRequest,
) -> Result<ReplyDeliveryReservation, BotStateDbError> {
    let transaction = immediate(connection)?;
    if let Some(body) = transaction
        .query_row(
            "SELECT body FROM bot_reply_delivery WHERE reply_id=?1 OR idempotency_key=?2",
            params![request.reply_id, request.idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        let existing: BotReplyDeliveryRequest = decode(&body)?;
        if existing.reply_id != request.reply_id
            || existing.idempotency_key != request.idempotency_key
        {
            return Ok(ReplyDeliveryReservation::Conflict);
        }
        if existing == *request {
            complete_reply_source_event(&transaction, request)?;
            let receipt = reply_delivery_receipt(&transaction, &existing)?;
            transaction.commit()?;
            return Ok(ReplyDeliveryReservation::Existing(receipt));
        }
        if !reply_parts_all_pending(&transaction, &existing)? {
            return Ok(ReplyDeliveryReservation::Conflict);
        }
        replace_pending_reply_parts(&transaction, &existing, request)?;
        complete_reply_source_event(&transaction, request)?;
        transaction.commit()?;
        return Ok(ReplyDeliveryReservation::Reserved);
    }
    transaction.execute(
        "INSERT INTO bot_reply_delivery(reply_id, idempotency_key, body) VALUES (?1, ?2, ?3)",
        params![request.reply_id, request.idempotency_key, encode(request)?],
    )?;
    for (index, part) in request.parts.iter().enumerate() {
        let delivery = reply_part_request(request, part);
        if !matches!(
            reserve_delivery_in_transaction(&transaction, &delivery)?,
            DeliveryReservation::Reserved
        ) {
            return Ok(ReplyDeliveryReservation::Conflict);
        }
        transaction.execute(
            "INSERT INTO bot_reply_delivery_part(reply_id, delivery_id, part_index)
             VALUES (?1, ?2, ?3)",
            params![
                request.reply_id,
                part.part_id,
                i64::try_from(index).map_err(|_| {
                    BotStateDbError::Invariant("reply part index exceeds SQLite integer".into())
                })?
            ],
        )?;
    }
    complete_reply_source_event(&transaction, request)?;
    transaction.commit()?;
    Ok(ReplyDeliveryReservation::Reserved)
}

fn reply_parts_all_pending(
    connection: &Connection,
    request: &BotReplyDeliveryRequest,
) -> Result<bool, BotStateDbError> {
    for part in &request.parts {
        let Some(receipt) = delivery_receipt(connection, &part.part_id)? else {
            return Ok(false);
        };
        if receipt.status != DeliveryStatus::Pending {
            return Ok(false);
        }
    }
    Ok(true)
}

fn replace_pending_reply_parts(
    transaction: &Transaction<'_>,
    existing: &BotReplyDeliveryRequest,
    request: &BotReplyDeliveryRequest,
) -> Result<(), BotStateDbError> {
    for part in &existing.parts {
        transaction.execute(
            "DELETE FROM bot_delivery_attempt WHERE delivery_id=?1",
            params![part.part_id],
        )?;
        transaction.execute(
            "DELETE FROM bot_delivery_receipt WHERE delivery_id=?1",
            params![part.part_id],
        )?;
        transaction.execute(
            "DELETE FROM bot_reply_delivery_part WHERE delivery_id=?1",
            params![part.part_id],
        )?;
        transaction.execute(
            "DELETE FROM bot_delivery_request WHERE delivery_id=?1",
            params![part.part_id],
        )?;
    }
    transaction.execute(
        "UPDATE bot_reply_delivery SET idempotency_key=?2, body=?3 WHERE reply_id=?1",
        params![request.reply_id, request.idempotency_key, encode(request)?],
    )?;
    for (index, part) in request.parts.iter().enumerate() {
        let delivery = reply_part_request(request, part);
        if !matches!(
            reserve_delivery_in_transaction(transaction, &delivery)?,
            DeliveryReservation::Reserved
        ) {
            return Err(BotStateDbError::Invariant(
                "pending reply part replacement collided with another delivery".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO bot_reply_delivery_part(reply_id, delivery_id, part_index)
             VALUES (?1, ?2, ?3)",
            params![
                request.reply_id,
                part.part_id,
                i64::try_from(index).map_err(|_| {
                    BotStateDbError::Invariant("reply part index exceeds SQLite integer".into())
                })?
            ],
        )?;
    }
    Ok(())
}

fn complete_reply_source_event(
    transaction: &Transaction<'_>,
    request: &BotReplyDeliveryRequest,
) -> Result<(), BotStateDbError> {
    if let Some(binding_key) = request.source_binding_key.as_deref() {
        transaction.execute(
            "UPDATE bot_agent_event SET status='completed'
             WHERE binding_key=?1 AND event_id=?2 AND status='pending'",
            params![binding_key, request.source_event_id],
        )?;
    }
    Ok(())
}

fn reply_delivery_receipt_by_id(
    connection: &Connection,
    reply_id: &str,
) -> Result<Option<BotReplyDeliveryReceipt>, BotStateDbError> {
    let request = optional_body::<BotReplyDeliveryRequest>(
        connection,
        "SELECT body FROM bot_reply_delivery WHERE reply_id=?1",
        reply_id,
    )?;
    request
        .map(|request| reply_delivery_receipt(connection, &request))
        .transpose()
}

fn reply_delivery_receipt(
    connection: &Connection,
    request: &BotReplyDeliveryRequest,
) -> Result<BotReplyDeliveryReceipt, BotStateDbError> {
    let part_receipts = request
        .parts
        .iter()
        .map(|part| {
            delivery_receipt(connection, &part.part_id)?.ok_or_else(|| {
                BotStateDbError::Invariant(format!("reply part receipt missing: {}", part.part_id))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BotReplyDeliveryReceipt {
        reply_id: request.reply_id.clone(),
        idempotency_key: request.idempotency_key.clone(),
        part_receipts,
    })
}

fn is_reply_part(connection: &Connection, delivery_id: &str) -> Result<bool, BotStateDbError> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM bot_reply_delivery_part WHERE delivery_id=?1",
            params![delivery_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

#[async_trait]
impl ReplyDeliveryRepository for BotStateDbRepository {
    async fn reserve_reply(
        &self,
        request: &BotReplyDeliveryRequest,
    ) -> Result<Option<BotReplyDeliveryReceipt>, DeliveryError> {
        let request = request.clone();
        match self
            .call_sync(true, move |connection| {
                reserve_reply_delivery(connection, &request)
            })
            .map_err(delivery_error)?
        {
            ReplyDeliveryReservation::Reserved => Ok(None),
            ReplyDeliveryReservation::Existing(receipt) => Ok(Some(receipt)),
            ReplyDeliveryReservation::Conflict => Err(DeliveryError::Conflict),
        }
    }

    async fn reply_receipt(
        &self,
        reply_id: &str,
    ) -> Result<BotReplyDeliveryReceipt, DeliveryError> {
        let reply_id = reply_id.to_owned();
        self.call_sync(false, move |connection| {
            reply_delivery_receipt_by_id(connection, &reply_id)
        })
        .map_err(delivery_error)?
        .ok_or(DeliveryError::NotFound)
    }

    async fn claim_due_reply_part_id(
        &self,
        now_unix_ms: u64,
    ) -> Result<Option<String>, DeliveryError> {
        self.call_sync(true, move |connection| {
            claim_due_reply_parts(connection, now_unix_ms)
        })
        .map(|ids| ids.into_iter().next())
        .map_err(delivery_error)
    }

    async fn is_reply_part(&self, delivery_id: &str) -> Result<bool, DeliveryError> {
        let delivery_id = delivery_id.to_owned();
        self.call_sync(false, move |connection| {
            is_reply_part(connection, &delivery_id)
        })
        .map_err(delivery_error)
    }
}
