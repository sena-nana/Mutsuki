use std::fmt;
use std::marker::PhantomData;

use mutsuki_runtime_contracts::{RuntimeError, ScalarValue, TaskHandle, TaskOutcome};
use serde::de::DeserializeOwned;

use crate::{RuntimeFailure, RuntimeResult, SdkProtocol};

pub trait SdkProtocolOutput: SdkProtocol {
    type Output: DeserializeOwned;
}

pub struct TypedTaskHandle<P> {
    handle: TaskHandle,
    _marker: PhantomData<fn() -> P>,
}

impl<P> Clone for TypedTaskHandle<P> {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            _marker: PhantomData,
        }
    }
}

impl<P> fmt::Debug for TypedTaskHandle<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedTaskHandle")
            .field("handle", &self.handle)
            .finish()
    }
}

impl<P> PartialEq for TypedTaskHandle<P> {
    fn eq(&self, other: &Self) -> bool {
        self.handle == other.handle
    }
}

impl<P> Eq for TypedTaskHandle<P> {}

impl<P: SdkProtocol> TypedTaskHandle<P> {
    pub fn try_from_handle(handle: TaskHandle) -> RuntimeResult<Self> {
        if handle.protocol_id != P::PROTOCOL_ID {
            return Err(protocol_mismatch(&handle, P::PROTOCOL_ID));
        }
        Ok(Self {
            handle,
            _marker: PhantomData,
        })
    }

    pub fn as_handle(&self) -> &TaskHandle {
        &self.handle
    }

    pub fn into_handle(self) -> TaskHandle {
        self.handle
    }
}

impl<P> From<TypedTaskHandle<P>> for TaskHandle {
    fn from(value: TypedTaskHandle<P>) -> Self {
        value.handle
    }
}

pub struct TypedTaskOutcome<P> {
    outcome: TaskOutcome,
    _marker: PhantomData<fn() -> P>,
}

impl<P> Clone for TypedTaskOutcome<P> {
    fn clone(&self) -> Self {
        Self {
            outcome: self.outcome.clone(),
            _marker: PhantomData,
        }
    }
}

impl<P> fmt::Debug for TypedTaskOutcome<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedTaskOutcome")
            .field("outcome", &self.outcome)
            .finish()
    }
}

impl<P> PartialEq for TypedTaskOutcome<P> {
    fn eq(&self, other: &Self) -> bool {
        self.outcome == other.outcome
    }
}

impl<P> TypedTaskOutcome<P> {
    pub(crate) fn from_outcome(outcome: TaskOutcome) -> Self {
        Self {
            outcome,
            _marker: PhantomData,
        }
    }

    pub fn as_outcome(&self) -> &TaskOutcome {
        &self.outcome
    }

    pub fn into_outcome(self) -> TaskOutcome {
        self.outcome
    }

    pub fn decode<T: DeserializeOwned>(&self) -> RuntimeResult<T> {
        match &self.outcome {
            TaskOutcome::Completed {
                output: Some(output),
                ..
            } => serde_json::from_value(output.clone()).map_err(decode_error),
            TaskOutcome::Completed { task_id, .. } => Err(decode_failed(format!(
                "task.outcome.{task_id}.completed_output_missing"
            ))),
            TaskOutcome::Failed { error, .. } => Err(RuntimeFailure::new(error.clone())),
            TaskOutcome::Cancelled { task_id, .. } => {
                Err(decode_failed(format!("task.outcome.{task_id}.cancelled")))
            }
            TaskOutcome::Expired { task_id, .. } => {
                Err(decode_failed(format!("task.outcome.{task_id}.expired")))
            }
            TaskOutcome::DeadLetter { task_id, .. } => {
                Err(decode_failed(format!("task.outcome.{task_id}.dead_letter")))
            }
        }
    }
}

impl<P: SdkProtocolOutput> TypedTaskOutcome<P> {
    pub fn decode_output(&self) -> RuntimeResult<P::Output> {
        self.decode()
    }
}

impl<P> From<TypedTaskOutcome<P>> for TaskOutcome {
    fn from(value: TypedTaskOutcome<P>) -> Self {
        value.outcome
    }
}

pub(crate) fn protocol_mismatch(handle: &TaskHandle, expected: &str) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        "task.protocol_mismatch",
        "runtime.sdk",
        format!("task.handle.{}", handle.task_id),
    );
    error.evidence.insert(
        "expected_protocol_id".into(),
        ScalarValue::String(expected.into()),
    );
    error.evidence.insert(
        "actual_protocol_id".into(),
        ScalarValue::String(handle.protocol_id.to_string()),
    );
    RuntimeFailure::new(error)
}

fn decode_failed(route: impl Into<String>) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new("sdk.decode_failed", "runtime.sdk", route))
}

fn decode_error(error: serde_json::Error) -> RuntimeFailure {
    decode_failed(error.to_string())
}
