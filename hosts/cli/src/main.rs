use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use mutsuki_service_ipc::{
    ControlClient, ControlClientConfig, IpcTransport, default_control_endpoint,
};

#[derive(Parser)]
#[command(name = "mutsuki-cli")]
#[command(about = "Terminal control client for MutsukiServiceHost")]
struct Cli {
    #[arg(long, value_enum, default_value_t = default_transport())]
    transport: CliTransport,
    #[arg(long)]
    endpoint: Option<String>,
    #[arg(long)]
    home: Option<PathBuf>,
    #[arg(long, default_value = "mutsuki")]
    name: String,
    #[arg(long, env = "MUTSUKI_CONTROL_TOKEN")]
    token: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliTransport {
    NamedPipe,
    UnixSocket,
    TcpDebug,
}

impl From<CliTransport> for IpcTransport {
    fn from(value: CliTransport) -> Self {
        match value {
            CliTransport::NamedPipe => Self::NamedPipe,
            CliTransport::UnixSocket => Self::UnixSocket,
            CliTransport::TcpDebug => Self::TcpDebug,
        }
    }
}

#[cfg(windows)]
fn default_transport() -> CliTransport {
    CliTransport::NamedPipe
}

#[cfg(not(windows))]
fn default_transport() -> CliTransport {
    CliTransport::UnixSocket
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let transport: IpcTransport = cli.transport.into();
    let run_dir = cli
        .home
        .unwrap_or_else(|| PathBuf::from(".mutsuki"))
        .join("run");
    let endpoint = cli
        .endpoint
        .unwrap_or_else(|| default_control_endpoint(transport.clone(), &cli.name, &run_dir, None));
    let token = cli
        .token
        .ok_or_else(|| anyhow::anyhow!("control token is required"))?;
    let client = ControlClient::new(ControlClientConfig::new(transport, endpoint, token));
    mutsuki_cli_host::run(client).await
}
