use std::io::{Stdout, stdout};
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use mutsuki_service_control::{
    ControlErrorBody, ControlMethod, HealthReport, LogTailParams, LogTailResponse, ServiceStatus,
};
use mutsuki_service_ipc::ControlClient;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const LOG_POLL_INTERVAL: Duration = Duration::from_millis(500);
const STATUS_POLL_INTERVAL: Duration = Duration::from_secs(2);
const MAX_LOG_LINES: usize = 500;

pub async fn run(client: ControlClient) -> anyhow::Result<()> {
    let mut app = CliApp::default();
    refresh_status(&client, &mut app).await?;
    refresh_logs(&client, &mut app, Some(100)).await?;

    let mut terminal = TerminalSession::new()?;
    let mut last_log_poll = Instant::now();
    let mut last_status_poll = Instant::now();

    while !app.should_quit {
        terminal.terminal.draw(|frame| render(frame, &app))?;

        if last_log_poll.elapsed() >= LOG_POLL_INTERVAL {
            if let Err(error) = refresh_logs(&client, &mut app, None).await {
                app.last_error = Some(error.to_string());
            }
            last_log_poll = Instant::now();
        }
        if last_status_poll.elapsed() >= STATUS_POLL_INTERVAL {
            if let Err(error) = refresh_status(&client, &mut app).await {
                app.last_error = Some(error.to_string());
            }
            last_status_poll = Instant::now();
        }

        if event::poll(Duration::from_millis(50))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            match app.handle_key(key.code, key.modifiers) {
                AppAction::None => {}
                AppAction::Quit => app.should_quit = true,
                AppAction::Refresh => {
                    if let Err(error) = refresh_status(&client, &mut app).await {
                        app.last_error = Some(error.to_string());
                    }
                    if let Err(error) = refresh_logs(&client, &mut app, None).await {
                        app.last_error = Some(error.to_string());
                    }
                    last_log_poll = Instant::now();
                    last_status_poll = Instant::now();
                }
            }
        }
    }
    Ok(())
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn new() -> anyhow::Result<Self> {
        enable_raw_mode().context("enable terminal raw mode")?;
        let mut output = stdout();
        if let Err(error) = execute!(output, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error).context("enter alternate screen");
        }
        let terminal = Terminal::new(CrosstermBackend::new(output)).context("create terminal")?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Default)]
struct CliApp {
    status: Option<ServiceStatus>,
    health: Option<HealthReport>,
    logs: Vec<String>,
    log_cursor: Option<u64>,
    last_error: Option<String>,
    should_quit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppAction {
    None,
    Quit,
    Refresh,
}

impl CliApp {
    fn handle_key(&self, code: KeyCode, modifiers: KeyModifiers) -> AppAction {
        if code == KeyCode::Esc
            || code == KeyCode::Char('q')
            || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
        {
            AppAction::Quit
        } else if code == KeyCode::Char('r') {
            AppAction::Refresh
        } else {
            AppAction::None
        }
    }

    fn apply_log_tail(&mut self, response: LogTailResponse) {
        self.log_cursor = Some(response.cursor);
        self.logs
            .extend(response.entries.into_iter().map(|entry| entry.line));
        if self.logs.len() > MAX_LOG_LINES {
            self.logs.drain(..self.logs.len() - MAX_LOG_LINES);
        }
        self.last_error = None;
    }
}

fn render(frame: &mut ratatui::Frame<'_>, app: &CliApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let service = app.status.as_ref().map_or_else(
        || "service unavailable".to_string(),
        |status| {
            format!(
                "instance={} profile={} uptime={}s core={} plugins={} runners={}\nipc={}",
                status.instance_id,
                status.profile,
                status.uptime_ms / 1_000,
                status.core_running,
                status.plugin_count,
                status.runner_count,
                status.ipc_endpoint
            )
        },
    );
    frame.render_widget(
        Paragraph::new(service).block(Block::default().title("Service").borders(Borders::ALL)),
        chunks[0],
    );

    let health = app.health.as_ref().map_or_else(
        || "health unavailable".to_string(),
        |health| {
            let errors = if health.recent_errors.is_empty() {
                "none".to_string()
            } else {
                health.recent_errors.join(" | ")
            };
            format!(
                "core={} plugins={} runners={} event_sources={}\nrecent_errors={errors}",
                health.core, health.plugins, health.runners, health.event_sources
            )
        },
    );
    frame.render_widget(
        Paragraph::new(health).block(Block::default().title("Health").borders(Borders::ALL)),
        chunks[1],
    );

    let logs = app
        .logs
        .iter()
        .map(|line| Line::from(line.as_str()))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(logs)
            .block(Block::default().title("Logs").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        chunks[2],
    );

    let footer = app
        .last_error
        .as_deref()
        .map_or("R refresh | Q/Esc quit", |error| error);
    frame.render_widget(Paragraph::new(footer), chunks[3]);
}

async fn refresh_status(client: &ControlClient, app: &mut CliApp) -> anyhow::Result<()> {
    let status = request_control(client, ControlMethod::ServiceStatus, Value::Null).await?;
    let health = request_control(client, ControlMethod::HealthCheck, Value::Null).await?;
    app.status = Some(status);
    app.health = Some(health);
    app.last_error = None;
    Ok(())
}

async fn refresh_logs(
    client: &ControlClient,
    app: &mut CliApp,
    lines: Option<usize>,
) -> anyhow::Result<()> {
    let response = request_control(
        client,
        ControlMethod::LogTail,
        json!(LogTailParams {
            cursor: app.log_cursor,
            lines,
            filters: Default::default(),
        }),
    )
    .await?;
    app.apply_log_tail(response);
    Ok(())
}

async fn request_control<T: DeserializeOwned>(
    client: &ControlClient,
    method: ControlMethod,
    params: Value,
) -> anyhow::Result<T> {
    let response = client.request(method, params).await?;
    if !response.ok {
        return Err(control_error(response.error));
    }
    serde_json::from_value(response.result.unwrap_or(Value::Null)).map_err(Into::into)
}

fn control_error(error: Option<ControlErrorBody>) -> anyhow::Error {
    match error {
        Some(error) => anyhow::anyhow!("{}: {}", error.code, error.message),
        None => anyhow::anyhow!("control request failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_service_control::LogTailEntry;

    #[test]
    fn key_bindings_only_trigger_control_actions() {
        let app = CliApp::default();
        assert_eq!(
            app.handle_key(KeyCode::Char('r'), KeyModifiers::empty()),
            AppAction::Refresh
        );
        assert_eq!(
            app.handle_key(KeyCode::Char('q'), KeyModifiers::empty()),
            AppAction::Quit
        );
        assert_eq!(
            app.handle_key(KeyCode::Char('x'), KeyModifiers::empty()),
            AppAction::None
        );
    }

    #[test]
    fn log_tail_tracks_cursor_and_bounds_memory() {
        let mut app = CliApp::default();
        app.apply_log_tail(LogTailResponse {
            cursor: 42,
            entries: (0..MAX_LOG_LINES + 5)
                .map(|offset| LogTailEntry {
                    offset: offset as u64,
                    line: format!("line-{offset}"),
                })
                .collect(),
        });

        assert_eq!(app.log_cursor, Some(42));
        assert_eq!(app.logs.len(), MAX_LOG_LINES);
        assert_eq!(app.logs.first().map(String::as_str), Some("line-5"));
    }
}
