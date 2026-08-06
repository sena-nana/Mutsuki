use std::sync::Arc;

use mutsuki_service_control::{ControlCommand, ControlRequest, ControlResponse};
use tokio::sync::Mutex;

use crate::error::IpcResult;
use crate::session::{ControlClientConfig, ControlSession, request_oneshot};

#[derive(Clone)]
pub struct ControlClient {
    config: ControlClientConfig,
    session: Arc<Mutex<Option<ControlSession>>>,
}

impl ControlClient {
    pub fn new(config: ControlClientConfig) -> Self {
        Self {
            config,
            session: Arc::new(Mutex::new(None)),
        }
    }

    pub fn config(&self) -> &ControlClientConfig {
        &self.config
    }

    pub async fn connect(&self) -> IpcResult<()> {
        let mut guard = self.session.lock().await;
        if guard.is_none() {
            *guard = Some(ControlSession::connect(self.config.clone()).await?);
        }
        Ok(())
    }

    pub async fn request(&self, command: ControlCommand) -> IpcResult<ControlResponse> {
        self.send(ControlRequest::new(self.config.token.clone(), command))
            .await
    }

    /// Reuse a persistent session; reconnect once on disconnect.
    pub async fn send(&self, request: ControlRequest) -> IpcResult<ControlResponse> {
        match self.send_with_session(request.clone()).await {
            Ok(response) => Ok(response),
            Err(crate::error::IpcError::Closed | crate::error::IpcError::Io(_)) => {
                {
                    let mut guard = self.session.lock().await;
                    *guard = None;
                }
                self.send_with_session(request).await
            }
            Err(error) => Err(error),
        }
    }

    async fn send_with_session(&self, request: ControlRequest) -> IpcResult<ControlResponse> {
        self.connect().await?;
        let mut guard = self.session.lock().await;
        let session = guard.as_mut().expect("session connected");
        session.send(request).await
    }

    /// Explicit one-shot compatibility API for migration callers and benchmarks.
    pub async fn request_oneshot(&self, command: ControlCommand) -> IpcResult<ControlResponse> {
        request_oneshot(
            &self.config,
            ControlRequest::new(self.config.token.clone(), command),
        )
        .await
    }

    pub async fn close(&self) -> IpcResult<()> {
        let mut guard = self.session.lock().await;
        if let Some(session) = guard.take() {
            session.close().await?;
        }
        Ok(())
    }
}
