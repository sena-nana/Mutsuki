use std::time::Duration;

use mutsuki_link_core::TransportErrorKind;
use mutsuki_link_local::LocalConnection;
use serde::Serialize;
use serde::de::DeserializeOwned;

const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

/// Awaits queue capacity (no polling) within the frame budget.
pub(crate) async fn send_json(
    connection: &mut LocalConnection,
    value: &impl Serialize,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    tokio::time::timeout(FRAME_TIMEOUT, connection.send(bytes))
        .await
        .map_err(|_| "send timed out".to_string())?
        .map_err(|error| error.to_string())
}

/// Awaits the next frame (no polling) within the frame budget.
pub(crate) async fn recv_json<T: DeserializeOwned>(
    connection: &mut LocalConnection,
) -> Result<T, String> {
    match tokio::time::timeout(FRAME_TIMEOUT, connection.receive()).await {
        Err(_) => Err("receive timed out".into()),
        Ok(Ok(bytes)) => serde_json::from_slice(&bytes).map_err(|error| error.to_string()),
        Ok(Err(error)) if error.kind == TransportErrorKind::Closed => {
            Err("connection closed before frame".into())
        }
        Ok(Err(error)) => Err(error.to_string()),
    }
}

/// Flushes the final reply before the connection is dropped.
pub(crate) async fn finish(connection: &mut LocalConnection) {
    let _ = tokio::time::timeout(FRAME_TIMEOUT, connection.close_and_flush()).await;
}
