// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::map_unwrap_or,
    clippy::semicolon_if_nothing_returned,
    clippy::too_many_lines
)]

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use mutsuki_service_config::{ConfigOverrides, ServiceConfig};
use mutsuki_service_control::{
    ControlCommand, IdParam, PluginDeploymentClearParam, PluginDeploymentParam,
};

#[derive(Parser)]
#[command(name = "mutsuki-service")]
#[command(about = "MutsukiCore foreground daemon host and local control client")]
struct Cli {
    #[arg(long, global = true)]
    profile: Option<String>,
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[arg(long, env = "MUTSUKI_CONTROL_TOKEN", global = true)]
    token: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Run,
    Status,
    Stop,
    Health,
    Install {
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
        #[arg(long)]
        service_user: Option<String>,
    },
    Uninstall {
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
    },
    Start {
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
    },
    #[command(name = "windows-service-run", hide = true)]
    WindowsServiceRun,
    #[command(subcommand)]
    Plugin(PluginCommand),
    #[command(subcommand)]
    Runner(RunnerCommand),
    #[command(subcommand)]
    EventSource(EventSourceCommand),
    #[command(subcommand)]
    Task(TaskCommand),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ScopeArg {
    User,
    System,
}

impl From<ScopeArg> for mutsuki_service_daemon::DaemonScope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::User => Self::User,
            ScopeArg::System => Self::System,
        }
    }
}

#[derive(Subcommand)]
enum PluginCommand {
    List,
    Reload,
    Set { id: String, deployment: String },
    Clear { id: String },
}

#[derive(Subcommand)]
enum RunnerCommand {
    List,
    Restart { id: String },
    Stop { id: String },
}

#[derive(Subcommand)]
enum EventSourceCommand {
    List,
    Restart { id: String },
}

#[derive(Subcommand)]
enum TaskCommand {
    List,
    Cancel { id: String },
    Outcome { id: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Cli {
        profile,
        config: config_file,
        home,
        token,
        command,
    } = Cli::parse();
    let token_override = token.is_some();
    let config = ServiceConfig::load(ConfigOverrides {
        profile,
        config_file: config_file.clone(),
        home_dir: home,
        control_token: token,
    })?;

    match command {
        Command::Run => {
            let runtime = mutsuki_service_runtime::ServiceRuntime::start(config).await?;
            runtime.run_foreground().await?;
        }
        Command::Status => request_and_print(&config, ControlCommand::ServiceStatus).await?,
        Command::Stop => request_and_print(&config, ControlCommand::ServiceShutdown).await?,
        Command::Health => request_and_print(&config, ControlCommand::HealthCheck).await?,
        Command::Install {
            scope,
            service_user,
        } => {
            let launch = mutsuki_service_daemon::DaemonLaunchOptions::from_config(
                &config,
                config_file,
                token_override,
            )?;
            mutsuki_service_daemon::install_with_scope(
                &config,
                &launch,
                daemon_scope(scope),
                service_user.as_deref(),
            )?;
            print_daemon_action("installed", &config);
        }
        Command::Uninstall { scope } => {
            mutsuki_service_daemon::uninstall_with_scope(&config, daemon_scope(scope))?;
            print_daemon_action("uninstalled", &config);
        }
        Command::Start { scope } => {
            mutsuki_service_daemon::start_with_scope(&config, daemon_scope(scope))?;
            print_daemon_action("started", &config);
        }
        Command::WindowsServiceRun => {
            mutsuki_service_daemon::run_windows_service(config)?;
        }
        Command::Plugin(command) => match command {
            PluginCommand::List => request_and_print(&config, ControlCommand::PluginList).await?,
            PluginCommand::Reload => {
                request_and_print(&config, ControlCommand::PluginReload).await?
            }
            PluginCommand::Set { id, deployment } => {
                let deployment = parse_deployment(&deployment)?;
                request_and_print(
                    &config,
                    ControlCommand::PluginDeploymentSet(PluginDeploymentParam {
                        plugin_id: id,
                        deployment,
                    }),
                )
                .await?
            }
            PluginCommand::Clear { id } => {
                request_and_print(
                    &config,
                    ControlCommand::PluginDeploymentClear(PluginDeploymentClearParam {
                        plugin_id: id,
                    }),
                )
                .await?
            }
        },
        Command::Runner(command) => match command {
            RunnerCommand::List => request_and_print(&config, ControlCommand::RunnerList).await?,
            RunnerCommand::Restart { id } => {
                request_and_print(&config, ControlCommand::RunnerRestart(IdParam { id })).await?
            }
            RunnerCommand::Stop { id } => {
                request_and_print(&config, ControlCommand::RunnerStop(IdParam { id })).await?
            }
        },
        Command::EventSource(command) => match command {
            EventSourceCommand::List => {
                request_and_print(&config, ControlCommand::EventSourceList).await?
            }
            EventSourceCommand::Restart { id } => {
                request_and_print(&config, ControlCommand::EventSourceRestart(IdParam { id }))
                    .await?
            }
        },
        Command::Task(command) => match command {
            TaskCommand::List => request_and_print(&config, ControlCommand::TaskList).await?,
            TaskCommand::Cancel { id } => {
                request_and_print(&config, ControlCommand::TaskCancel(IdParam { id })).await?
            }
            TaskCommand::Outcome { id } => {
                request_and_print(&config, ControlCommand::TaskOutcome(IdParam { id })).await?
            }
        },
    }
    Ok(())
}

fn daemon_scope(scope: Option<ScopeArg>) -> mutsuki_service_daemon::DaemonScope {
    scope
        .map(Into::into)
        .unwrap_or_else(mutsuki_service_daemon::DaemonScope::platform_default)
}

fn parse_deployment(
    value: &str,
) -> anyhow::Result<mutsuki_runtime_contracts::PluginDeploymentKind> {
    use mutsuki_runtime_contracts::PluginDeploymentKind;
    match value.to_ascii_lowercase().as_str() {
        "builtin" => Ok(PluginDeploymentKind::Builtin),
        "abi" => Ok(PluginDeploymentKind::Abi),
        "wasm" => Ok(PluginDeploymentKind::Wasm),
        "process" => Ok(PluginDeploymentKind::Process),
        "python" => Ok(PluginDeploymentKind::Python),
        _ => anyhow::bail!("unsupported plugin deployment {value}"),
    }
}

fn print_daemon_action(action: &str, config: &ServiceConfig) {
    println!("{action} {}", mutsuki_service_daemon::service_name(config));
}

async fn request_and_print(config: &ServiceConfig, command: ControlCommand) -> anyhow::Result<()> {
    let client = mutsuki_service_ipc::ControlClient::new(config.into());
    let response = client.request(command).await?;
    let _ = client.close().await;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}
