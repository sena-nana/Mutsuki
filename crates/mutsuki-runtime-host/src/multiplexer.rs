mod pending;
mod threads;

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Sender, bounded};
use mutsuki_runtime_contracts::RuntimeError;
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_wire::WireLimits;

use self::pending::PendingShared;
use self::threads::{WriteCommand, reader_loop, writer_loop};

pub(crate) trait FrameCodec: Clone + Send + 'static {
    fn read_frame<R: BufRead>(&self, reader: &mut R) -> RuntimeResult<Option<Vec<u8>>>;
    fn response_id(&self, frame: &[u8]) -> RuntimeResult<u64>;
}

pub(crate) struct RequestMultiplexer<R, W> {
    shared: Arc<PendingShared>,
    work_writer: Option<Sender<WriteCommand>>,
    management_writer: Option<Sender<WriteCommand>>,
    reader_thread: Option<JoinHandle<R>>,
    writer_thread: Option<JoinHandle<W>>,
    next_request: AtomicU64,
    limits: Mutex<WireLimits>,
    response_timeout: Duration,
}

impl<R, W> RequestMultiplexer<R, W>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    pub(crate) fn new<C: FrameCodec>(
        reader: R,
        writer: W,
        codec: C,
        limits: WireLimits,
        response_timeout: Duration,
    ) -> Self {
        let work_capacity = limits
            .max_in_flight_requests
            .saturating_sub(limits.management_reserved_requests)
            .max(1);
        let management_capacity = limits.management_reserved_requests.max(1);
        let (work_writer, work_reader) = bounded(work_capacity);
        let (management_writer, management_reader) = bounded(management_capacity);
        let shared = Arc::new(PendingShared::default());
        let reader_shared = Arc::clone(&shared);
        let reader_thread = std::thread::Builder::new()
            .name("mutsuki-wire-reader".into())
            .spawn(move || reader_loop(reader, codec, reader_shared))
            .expect("spawn wire reader thread");
        let writer_shared = Arc::clone(&shared);
        let writer_thread = std::thread::Builder::new()
            .name("mutsuki-wire-writer".into())
            .spawn(move || writer_loop(writer, management_reader, work_reader, writer_shared))
            .expect("spawn wire writer thread");
        Self {
            shared,
            work_writer: Some(work_writer),
            management_writer: Some(management_writer),
            reader_thread: Some(reader_thread),
            writer_thread: Some(writer_thread),
            next_request: AtomicU64::new(0),
            limits: Mutex::new(limits),
            response_timeout,
        }
    }

    pub(crate) fn next_request_id(&self) -> RuntimeResult<u64> {
        let previous = self
            .next_request
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| transport_failure("request id space is exhausted"))?;
        Ok(previous + 1)
    }

    pub(crate) fn request(
        &self,
        request_id: u64,
        management: bool,
        frame: Vec<u8>,
    ) -> RuntimeResult<Vec<u8>> {
        let (response_sender, response_receiver) = bounded(1);
        let limits = *self
            .limits
            .lock()
            .map_err(|_| transport_failure("wire limits lock poisoned"))?;
        self.shared
            .insert(request_id, management, response_sender, limits)?;
        let (written_sender, written_receiver) = bounded(1);
        let command = WriteCommand {
            frame,
            written: written_sender,
        };
        let queue = if management {
            self.management_writer.as_ref()
        } else {
            self.work_writer.as_ref()
        }
        .ok_or_else(|| transport_failure("writer queue is closed"))?;
        if let Err(error) = queue.send_timeout(command, self.response_timeout) {
            self.shared.remove(request_id);
            return Err(transport_failure(&format!(
                "writer queue backpressure or disconnect: {error}"
            )));
        }
        match written_receiver.recv_timeout(self.response_timeout) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(error) => {
                let failure = transport_error(&format!("writer completion timeout: {error}"));
                self.shared.fail(failure.clone());
                return Err(RuntimeFailure::new(failure));
            }
        }
        match response_receiver.recv_timeout(self.response_timeout) {
            Ok(response) => response,
            Err(error) => {
                let failure = transport_error(&format!("response timeout or disconnect: {error}"));
                self.shared.fail(failure.clone());
                Err(RuntimeFailure::new(failure))
            }
        }
    }

    pub(crate) fn fail(&self, error: RuntimeError) {
        self.shared.fail(error);
    }

    pub(crate) fn set_limits(&self, limits: WireLimits) -> RuntimeResult<()> {
        limits
            .validate()
            .map_err(|error| transport_failure(&error.to_string()))?;
        *self
            .limits
            .lock()
            .map_err(|_| transport_failure("wire limits lock poisoned"))? = limits;
        Ok(())
    }

    pub(crate) fn into_inner(mut self) -> (R, W) {
        self.shared.close();
        self.work_writer.take();
        self.management_writer.take();
        let writer = self
            .writer_thread
            .take()
            .expect("wire writer thread missing")
            .join()
            .expect("wire writer thread panicked");
        let reader = self
            .reader_thread
            .take()
            .expect("wire reader thread missing")
            .join()
            .expect("wire reader thread panicked");
        (reader, writer)
    }
}

impl<R, W> Drop for RequestMultiplexer<R, W> {
    fn drop(&mut self) {
        self.shared.close();
        self.work_writer.take();
        self.management_writer.take();
    }
}

pub(super) fn transport_failure(message: &str) -> RuntimeFailure {
    RuntimeFailure::new(transport_error(message))
}

pub(super) fn transport_error(message: &str) -> RuntimeError {
    let mut error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        "wire_multiplexer",
        "wire.transport",
    );
    error.evidence.insert(
        "reason".into(),
        mutsuki_runtime_contracts::ScalarValue::String(message.into()),
    );
    error
}
