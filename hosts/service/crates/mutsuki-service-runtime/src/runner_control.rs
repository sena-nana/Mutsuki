//! Control commands for external runners and event sources.

#[allow(clippy::wildcard_imports)]
use super::*;

impl ServiceRuntimeInner {
    pub(crate) async fn runner_list(&self) -> ControlResponse {
        let snapshots = self.supervisor.list().await;
        ControlResponse::ok(ControlResult::RunnerList(to_control_runner_status(
            snapshots,
        )))
    }

    pub(crate) async fn runner_restart(&self, param: IdParam) -> ControlResponse {
        match self.supervisor.restart(&param.id).await {
            Ok(()) => ControlResponse::ok(ControlResult::RunnerRestart),
            Err(error) => ControlResponse::err(ControlError::Failed(error.to_string())),
        }
    }

    pub(crate) async fn runner_stop(&self, param: IdParam) -> ControlResponse {
        match self.supervisor.stop(&param.id).await {
            Ok(()) => ControlResponse::ok(ControlResult::RunnerStop),
            Err(error) => ControlResponse::err(ControlError::Failed(error.to_string())),
        }
    }

    pub(crate) fn event_source_list(&self) -> ControlResponse {
        ControlResponse::ok(ControlResult::EventSourceList(self.event_sources.list()))
    }

    pub(crate) async fn event_source_restart(&self, param: IdParam) -> ControlResponse {
        match self.event_sources.restart(&param.id).await {
            Ok(()) => ControlResponse::ok(ControlResult::EventSourceRestart),
            Err(error) => ControlResponse::err(ControlError::Failed(error)),
        }
    }
}
