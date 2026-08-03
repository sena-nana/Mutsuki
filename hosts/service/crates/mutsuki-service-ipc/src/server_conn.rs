use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mutsuki_service_control::{
    ControlError, ControlHandler, ControlMethod, ControlRequest, ControlResponse,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, OwnedMutexGuard, Semaphore, watch};
use tokio::task::JoinHandle;

use crate::codec::{ControlRequestBody, encode_binary_response_with_scratch};
use crate::error::{IpcError, IpcResult};
use crate::frame::{FrameFlags, OPCODE_CANCEL};
use crate::io::{read_frame_prefix, read_payload_or_discard, write_all_flush};
use crate::limits::ControlIpcProfile;

struct PendingEntry {
    abort: tokio::sync::watch::Sender<bool>,
    task: JoinHandle<()>,
}

pub async fn serve_stream<S>(
    stream: S,
    handler: Arc<dyn ControlHandler>,
    profile: ControlIpcProfile,
    drain_rx: watch::Receiver<bool>,
) -> IpcResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (reader, writer) = tokio::io::split(stream);
    serve_binary(
        reader,
        Arc::new(Mutex::new(writer)),
        handler,
        profile,
        drain_rx,
    )
    .await
}

async fn serve_binary<R, W>(
    mut reader: R,
    writer: Arc<Mutex<W>>,
    handler: Arc<dyn ControlHandler>,
    profile: ControlIpcProfile,
    mut drain_rx: watch::Receiver<bool>,
) -> IpcResult<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let limits = profile.limits;
    let idle = Duration::from_millis(limits.idle_timeout_ms.max(1));
    let mutate_lock = Arc::new(Mutex::new(()));
    let pending = Arc::new(Mutex::new(HashMap::<u64, PendingEntry>::new()));
    let pending_slots = Arc::new(Semaphore::new(limits.max_in_flight));
    let mut header_buf =
        [0_u8; crate::frame::BINARY_LENGTH_PREFIX_LEN + crate::frame::BINARY_HEADER_LEN];
    let response_encode = Arc::new(Mutex::new((Vec::new(), Vec::new())));

    loop {
        if *drain_rx.borrow() {
            wait_pending_drain(&pending).await;
            return Ok(());
        }
        let prefix = tokio::select! {
            biased;
            changed = drain_rx.changed() => {
                if changed.is_err() || *drain_rx.borrow() {
                    wait_pending_drain(&pending).await;
                    return Ok(());
                }
                continue;
            }
            frame = tokio::time::timeout(idle, read_frame_prefix(&mut reader, limits, &mut header_buf)) => {
                match frame {
                    Ok(Ok(Some(prefix))) => prefix,
                    Ok(Ok(None)) => {
                        wait_pending_drain(&pending).await;
                        return Ok(());
                    }
                    Ok(Err(error)) => return Err(error),
                    Err(_) => {
                        wait_pending_drain(&pending).await;
                        return Ok(());
                    }
                }
            }
        };

        let (declared, header) = prefix;
        if header.flags.contains(FrameFlags::CANCEL) || header.opcode == OPCODE_CANCEL {
            let _ = read_payload_or_discard(&mut reader, declared, &header, false).await?;
            cancel_pending(&pending, header.request_id).await;
            continue;
        }
        if !header.flags.contains(FrameFlags::REQUEST) {
            let _ = read_payload_or_discard(&mut reader, declared, &header, false).await?;
            return Err(IpcError::UnknownFlags(header.flags.bits()));
        }
        if *drain_rx.borrow() {
            let _ = read_payload_or_discard(&mut reader, declared, &header, false).await?;
            return Err(IpcError::Draining);
        }

        let permit = match pending_slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let _ = read_payload_or_discard(&mut reader, declared, &header, false).await?;
                let method =
                    ControlMethod::from_opcode(header.opcode).unwrap_or(ControlMethod::HealthCheck);
                let response = ControlResponse::err(ControlError::Failed(format!(
                    "pending request limit exceeded ({})",
                    limits.max_in_flight
                )));
                let mut encode_buf = Vec::new();
                let mut payload_buf = Vec::new();
                encode_binary_response_with_scratch(
                    header.request_id,
                    method,
                    &response,
                    limits,
                    &mut encode_buf,
                    &mut payload_buf,
                )?;
                let mut guard = writer.lock().await;
                write_all_flush(&mut *guard, &encode_buf).await?;
                continue;
            }
        };

        let Some(payload) = read_payload_or_discard(&mut reader, declared, &header, true).await?
        else {
            drop(permit);
            continue;
        };
        let method = match ControlMethod::from_opcode(header.opcode) {
            Some(method) => method,
            None => {
                drop(permit);
                return Err(IpcError::UnknownOpcode(header.opcode));
            }
        };
        let body: ControlRequestBody = {
            let mut deserializer = rmp_serde::Deserializer::new(payload.as_slice());
            deserializer.set_max_depth(limits.max_msgpack_nesting_depth);
            match serde::Deserialize::deserialize(&mut deserializer) {
                Ok(body) => body,
                Err(error) => {
                    drop(permit);
                    let response =
                        ControlResponse::err(ControlError::BadRequest(error.to_string()));
                    let mut encode_buf = Vec::new();
                    let mut payload_buf = Vec::new();
                    encode_binary_response_with_scratch(
                        header.request_id,
                        method,
                        &response,
                        limits,
                        &mut encode_buf,
                        &mut payload_buf,
                    )?;
                    let mut guard = writer.lock().await;
                    write_all_flush(&mut *guard, &encode_buf).await?;
                    continue;
                }
            }
        };
        let request = ControlRequest {
            token: body.token,
            method,
            params: body.params,
        };
        let request_id = header.request_id;
        let (abort_tx, abort_rx) = watch::channel(false);
        let handler = handler.clone();
        let writer = writer.clone();
        let mutate_lock = mutate_lock.clone();
        let pending_map = pending.clone();
        let response_encode = response_encode.clone();
        let task = tokio::spawn(async move {
            let _permit = permit;
            let response = dispatch_request(handler, request, mutate_lock, abort_rx).await;
            {
                let mut buffers = response_encode.lock().await;
                let (encode_buf, payload_buf) = &mut *buffers;
                if encode_binary_response_with_scratch(
                    request_id,
                    method,
                    &response,
                    limits,
                    encode_buf,
                    payload_buf,
                )
                .is_ok()
                {
                    let mut guard = writer.lock().await;
                    let _ = write_all_flush(&mut *guard, encode_buf).await;
                }
            }
            let mut map = pending_map.lock().await;
            map.remove(&request_id);
        });
        pending.lock().await.insert(
            request_id,
            PendingEntry {
                abort: abort_tx,
                task,
            },
        );
    }
}

async fn dispatch_request(
    handler: Arc<dyn ControlHandler>,
    request: ControlRequest,
    mutate_lock: Arc<Mutex<()>>,
    mut abort_rx: watch::Receiver<bool>,
) -> ControlResponse {
    let _guard: Option<OwnedMutexGuard<()>> = if request.method.is_mutating() {
        Some(mutate_lock.lock_owned().await)
    } else {
        None
    };
    let work = handler.handle(request);
    tokio::select! {
        response = work => response,
        _ = abort_rx.changed() => {
            ControlResponse::err(ControlError::Failed("request cancelled".into()))
        }
    }
}

async fn cancel_pending(pending: &Arc<Mutex<HashMap<u64, PendingEntry>>>, request_id: u64) {
    let mut map = pending.lock().await;
    if let Some(entry) = map.remove(&request_id) {
        let _ = entry.abort.send(true);
        entry.task.abort();
    }
}

async fn wait_pending_drain(pending: &Arc<Mutex<HashMap<u64, PendingEntry>>>) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let empty = {
            let map = pending.lock().await;
            map.is_empty()
        };
        if empty || tokio::time::Instant::now() >= deadline {
            let mut map = pending.lock().await;
            for (_, entry) in map.drain() {
                let _ = entry.abort.send(true);
                entry.task.abort();
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
