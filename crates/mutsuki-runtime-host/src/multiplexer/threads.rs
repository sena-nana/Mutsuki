use crossbeam_channel::{Receiver, Sender, TryRecvError};
use mutsuki_runtime_core::RuntimeResult;
use std::io::Write;
use std::sync::Arc;

use super::pending::PendingShared;
use super::{FrameCodec, transport_error, transport_failure};

pub(super) struct WriteCommand {
    pub(super) frame: Vec<u8>,
    pub(super) written: Sender<RuntimeResult<()>>,
}

pub(super) fn reader_loop<R, C>(mut reader: R, codec: C, shared: Arc<PendingShared>) -> R
where
    R: std::io::BufRead,
    C: FrameCodec,
{
    loop {
        if !shared.wait_until_ready_or_closed() {
            return reader;
        }
        match codec.read_frame(&mut reader) {
            Ok(Some(frame)) => match codec.response_id(&frame) {
                Ok(request_id) if request_id != 0 => match shared.take(request_id) {
                    Some(sender) => {
                        let _ = sender.send(Ok(frame));
                    }
                    None => {
                        shared.fail(transport_error("unknown, duplicate or late response id"));
                        return reader;
                    }
                },
                Ok(_) => {
                    shared.fail(transport_error("zero response id"));
                    return reader;
                }
                Err(error) => {
                    shared.fail(error.error().clone());
                    return reader;
                }
            },
            Ok(None) => {
                shared.fail(transport_error("unexpected EOF"));
                return reader;
            }
            Err(error) => {
                shared.fail(error.error().clone());
                return reader;
            }
        }
    }
}

pub(super) fn writer_loop<W: Write>(
    mut writer: W,
    management: Receiver<WriteCommand>,
    work: Receiver<WriteCommand>,
    shared: Arc<PendingShared>,
) -> W {
    let mut management_open = true;
    let mut work_open = true;
    let mut prefer_management = true;
    while management_open || work_open {
        if management_open && prefer_management {
            match management.try_recv() {
                Ok(command) => {
                    if !write_command(&mut writer, command, &shared) {
                        return writer;
                    }
                    prefer_management = false;
                    continue;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => management_open = false,
            }
        }
        if work_open {
            match work.try_recv() {
                Ok(command) => {
                    if !write_command(&mut writer, command, &shared) {
                        return writer;
                    }
                    prefer_management = true;
                    continue;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => work_open = false,
            }
        }
        if management_open {
            match management.try_recv() {
                Ok(command) => {
                    if !write_command(&mut writer, command, &shared) {
                        return writer;
                    }
                    prefer_management = false;
                    continue;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => management_open = false,
            }
        }
        match (management_open, work_open) {
            (true, true) => crossbeam_channel::select! {
                recv(management) -> command => match command {
                    Ok(command) => {
                        if !write_command(&mut writer, command, &shared) {
                            return writer;
                        }
                        prefer_management = false;
                    }
                    Err(_) => management_open = false,
                },
                recv(work) -> command => match command {
                    Ok(command) => {
                        if !write_command(&mut writer, command, &shared) {
                            return writer;
                        }
                        prefer_management = true;
                    }
                    Err(_) => work_open = false,
                },
            },
            (true, false) => match management.recv() {
                Ok(command) => {
                    if !write_command(&mut writer, command, &shared) {
                        return writer;
                    }
                    prefer_management = false;
                }
                Err(_) => management_open = false,
            },
            (false, true) => match work.recv() {
                Ok(command) => {
                    if !write_command(&mut writer, command, &shared) {
                        return writer;
                    }
                    prefer_management = true;
                }
                Err(_) => work_open = false,
            },
            (false, false) => {}
        }
    }
    writer
}

fn write_command<W: Write>(writer: &mut W, command: WriteCommand, shared: &PendingShared) -> bool {
    let result = writer
        .write_all(&command.frame)
        .and_then(|()| writer.flush())
        .map_err(|error| transport_failure(&format!("writer failure: {error}")));
    let succeeded = result.is_ok();
    if let Err(error) = &result {
        shared.fail(error.error().clone());
    }
    let _ = command.written.send(result);
    succeeded
}
