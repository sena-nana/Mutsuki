// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_possible_truncation,
    clippy::default_trait_access,
    clippy::manual_let_else,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::return_self_not_must_use,
    clippy::semicolon_if_nothing_returned,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::unused_async
)]

mod client;
mod codec;
mod error;
mod frame;
mod io;
mod limits;
mod server_conn;
mod session;
mod transport;

use std::path::Path;

pub use client::ControlClient;
pub use error::{IpcError, IpcResult};
pub use frame::{BINARY_HEADER_LEN, BINARY_LENGTH_PREFIX_LEN, CONTROL_WIRE_MAGIC};
pub use limits::{ControlIpcLimits, ControlIpcProfile};
pub use mutsuki_service_config::{IpcCodec, IpcTransport};
pub use session::{ControlClientConfig, ControlSession, request_oneshot};
pub use transport::{IpcServer, start_server};

pub fn default_control_endpoint(
    transport: IpcTransport,
    name: &str,
    run_dir: &Path,
    tcp_debug_addr: Option<&str>,
) -> String {
    match transport {
        IpcTransport::NamedPipe => name.to_string(),
        IpcTransport::UnixSocket => run_dir
            .join(format!("{name}.sock"))
            .to_string_lossy()
            .into_owned(),
        IpcTransport::TcpDebug => tcp_debug_addr.unwrap_or("127.0.0.1:7687").to_string(),
    }
}

#[cfg(test)]
mod tests;
