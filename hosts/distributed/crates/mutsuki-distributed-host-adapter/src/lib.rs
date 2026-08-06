//! Adapter from the existing local `ServiceHost` control API to the external
//! distributed sidecar. It does not add a distributed execution path to Host.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use mutsuki_distributed_contracts::{
    DistributedError, DistributedErrorKind, LocalTaskOutcome, LocalTaskSnapshot,
};
use mutsuki_runtime_contracts::{RuntimeEvent, TaskBatch, TaskHandle};
use mutsuki_service_control::{
    ControlCommand, ControlErrorCode, ControlResponse, ControlResult, IdParam,
    TaskEventsAfterParam, TaskSnapshot, TaskSubmitBatchParam,
};
use mutsuki_service_ipc::{ControlClient, ControlClientConfig, IpcTransport};
use std::future::Future;
use std::pin::Pin;

macro_rules! control_result {
    ($result:expr, $variant:ident) => {
        match $result {
            ControlResult::$variant(value) => Ok(value),
            _ => Err(protocol_error()),
        }
    };
}

pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, DistributedError>> + Send + 'a>>;

pub trait HostAdapter: Send + Sync {
    fn submit_batch(&self, batch: TaskBatch) -> HostFuture<'_, Vec<TaskHandle>>;
    fn cancel(&self, handle: &TaskHandle) -> HostFuture<'_, ()>;
    fn snapshots(&self) -> HostFuture<'_, Vec<LocalTaskSnapshot>>;
    fn outcome(&self, handle: &TaskHandle) -> HostFuture<'_, Option<LocalTaskOutcome>>;
    fn events_after(&self, sequence: u64, limit: usize) -> HostFuture<'_, Vec<RuntimeEvent>>;
    fn begin_drain(&self) -> HostFuture<'_, ()>;
    fn health(&self) -> HostFuture<'_, String>;
}

#[derive(Clone)]
pub struct ServiceHostAdapter {
    client: ControlClient,
}

impl std::fmt::Debug for ServiceHostAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceHostAdapter").finish_non_exhaustive()
    }
}

impl ServiceHostAdapter {
    pub fn new(config: ControlClientConfig) -> Self {
        Self {
            client: ControlClient::new(config),
        }
    }

    pub fn local_socket(
        endpoint: impl Into<String>,
        token: impl Into<String>,
    ) -> ServiceHostAdapter {
        #[cfg(windows)]
        let transport = IpcTransport::NamedPipe;
        #[cfg(unix)]
        let transport = IpcTransport::UnixSocket;
        Self::new(ControlClientConfig::new(transport, endpoint, token))
    }

    async fn request(&self, command: ControlCommand) -> Result<ControlResult, DistributedError> {
        let response = self
            .client
            .request(command)
            .await
            .map_err(|_| host_unavailable())?;
        decode_response(response)
    }
}

impl HostAdapter for ServiceHostAdapter {
    fn submit_batch(&self, batch: TaskBatch) -> HostFuture<'_, Vec<TaskHandle>> {
        Box::pin(async move {
            let response = control_result!(
                self.request(ControlCommand::TaskSubmitBatch(TaskSubmitBatchParam {
                    batch,
                }))
                .await?,
                TaskSubmitBatch
            )?;
            Ok(response.handles)
        })
    }

    fn cancel(&self, handle: &TaskHandle) -> HostFuture<'_, ()> {
        let id = handle.task_id.clone();
        Box::pin(async move {
            match self
                .request(ControlCommand::TaskCancel(IdParam { id }))
                .await?
            {
                ControlResult::TaskCancel => Ok(()),
                _ => Err(protocol_error()),
            }
        })
    }

    fn snapshots(&self) -> HostFuture<'_, Vec<LocalTaskSnapshot>> {
        Box::pin(async move {
            let snapshots =
                control_result!(self.request(ControlCommand::TaskList).await?, TaskList)?;
            Ok(snapshots.into_iter().map(map_snapshot).collect())
        })
    }

    fn outcome(&self, handle: &TaskHandle) -> HostFuture<'_, Option<LocalTaskOutcome>> {
        let id = handle.task_id.clone();
        Box::pin(async move {
            let outcome = control_result!(
                self.request(ControlCommand::TaskOutcome(IdParam { id }))
                    .await?,
                TaskOutcome
            )?;
            Ok(Some(LocalTaskOutcome {
                task_id: outcome.task_id,
                status: outcome.status,
                output_ref: outcome.output_ref,
                reason: outcome.reason,
                error_code: outcome.error_code,
            }))
        })
    }

    fn events_after(&self, sequence: u64, limit: usize) -> HostFuture<'_, Vec<RuntimeEvent>> {
        Box::pin(async move {
            let page = control_result!(
                self.request(ControlCommand::TaskEventsAfter(TaskEventsAfterParam {
                    sequence,
                    limit,
                }))
                .await?,
                TaskEventsAfter
            )?;
            Ok(page.events)
        })
    }

    fn begin_drain(&self) -> HostFuture<'_, ()> {
        Box::pin(async move {
            let response = control_result!(
                self.request(ControlCommand::CoreBeginDrain).await?,
                CoreBeginDrain
            )?;
            if response.state != "draining" {
                return Err(DistributedError::new(
                    DistributedErrorKind::HostUnavailable,
                    "local Host did not enter draining state",
                ));
            }
            Ok(())
        })
    }

    fn health(&self) -> HostFuture<'_, String> {
        Box::pin(async move {
            let core =
                control_result!(self.request(ControlCommand::CoreStatus).await?, CoreStatus)?;
            if !core.running {
                return Err(host_unavailable());
            }
            let health = control_result!(
                self.request(ControlCommand::HealthCheck).await?,
                HealthCheck
            )?;
            Ok(health.core)
        })
    }
}

fn decode_response(response: ControlResponse) -> Result<ControlResult, DistributedError> {
    match response {
        ControlResponse::Ok(result) => Ok(result),
        ControlResponse::Error(error) => {
            let kind = if error.code == ControlErrorCode::Unsupported {
                DistributedErrorKind::Incompatible
            } else {
                DistributedErrorKind::HostUnavailable
            };
            Err(DistributedError::new(
                kind,
                "local Host rejected the control request",
            ))
        }
    }
}

fn map_snapshot(snapshot: TaskSnapshot) -> LocalTaskSnapshot {
    LocalTaskSnapshot {
        task_id: snapshot.task_id,
        protocol_id: snapshot.protocol_id,
        status: snapshot.status,
        registry_generation: snapshot.registry_generation,
        runner_id: snapshot.claimed_by.or(snapshot.owner_runner),
        lease_id: snapshot.lease_id,
    }
}

const fn host_unavailable() -> DistributedError {
    DistributedError::new(
        DistributedErrorKind::HostUnavailable,
        "local Host is unavailable",
    )
}

const fn protocol_error() -> DistributedError {
    DistributedError::new(
        DistributedErrorKind::Protocol,
        "local Host control response is invalid",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_service_control::{ControlError, ControlResponse, ServiceStatus};

    #[test]
    fn unsupported_control_surface_is_reported_as_incompatible() {
        let response = ControlResponse::err(ControlError::Unsupported("task_submit_batch".into()));
        assert_eq!(
            decode_response(response).unwrap_err().kind,
            DistributedErrorKind::Incompatible
        );
    }

    #[test]
    fn mismatched_success_response_is_a_protocol_error() {
        let response = ControlResponse::ok(ControlResult::ServiceStatus(ServiceStatus {
            instance_id: "test".into(),
            profile: "test".into(),
            uptime_ms: 0,
            ipc_endpoint: "test".into(),
            core_running: true,
            plugin_count: 0,
            runner_count: 0,
        }));
        let result = decode_response(response).expect("typed response");
        assert_eq!(
            control_result!(result, CoreStatus).unwrap_err().kind,
            DistributedErrorKind::Protocol
        );
    }
}
