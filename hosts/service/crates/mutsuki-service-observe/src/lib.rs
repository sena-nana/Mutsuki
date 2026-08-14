use std::fs::OpenOptions;
use std::io::Write;
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_service_config::ServiceConfig;
use tracing_subscriber::EnvFilter;

pub struct ObserveGuard {
    _file_guard: tracing_appender::non_blocking::WorkerGuard,
}

pub fn init_observe(config: &ServiceConfig) -> ObserveGuard {
    init_observe_with_listener(config, Arc::new(|| {}))
}

pub fn init_observe_with_listener(
    config: &ServiceConfig,
    changed: Arc<dyn Fn() + Send + Sync>,
) -> ObserveGuard {
    let file_appender =
        tracing_appender::rolling::never(&config.service.log_dir, &config.observe.log_file);
    let (file_writer, file_guard) = tracing_appender::non_blocking(NotifyingWriter {
        inner: file_appender,
        changed,
    });
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(file_writer)
        .json()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
    if config.observe.console {
        tracing::info!(
            instance_id = %config.service.instance_id,
            profile = %config.service.profile,
            "mutsuki service host starting"
        );
    }
    install_panic_hook(config.service.log_dir.join(&config.observe.panic_file));
    ObserveGuard {
        _file_guard: file_guard,
    }
}

struct NotifyingWriter<W> {
    inner: W,
    changed: Arc<dyn Fn() + Send + Sync>,
}

impl<W: Write> Write for NotifyingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(bytes)?;
        if written > 0 {
            (self.changed)();
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub fn install_panic_hook(path: PathBuf) {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = append_panic(&path, info.to_string());
        previous(info);
    }));
}

fn append_panic(path: &Path, message: String) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{message}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn log_listener_runs_after_bytes_are_written() {
        let notifications = Arc::new(AtomicUsize::new(0));
        let observed = notifications.clone();
        let mut writer = NotifyingWriter {
            inner: Vec::new(),
            changed: Arc::new(move || {
                observed.fetch_add(1, Ordering::SeqCst);
            }),
        };
        writer.write_all(b"entry\n").unwrap();
        assert_eq!(writer.inner, b"entry\n");
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
    }
}
