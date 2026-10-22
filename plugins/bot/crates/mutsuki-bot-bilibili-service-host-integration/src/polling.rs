//! `ServiceHost` event source for Bilibili polling: secret requirement, descriptor and health.
//! The poll loop itself is `mutsuki_plugin_bot_bilibili::run_bilibili_polling`; the cookie is
//! resolved by the HTTP effect per request, never loaded here.

use std::sync::{Arc, Mutex};

use mutsuki_plugin_bot_bilibili::{
    BilibiliPollerHealth, PLUGIN_ID, SharedBilibiliConfig, run_bilibili_polling,
};
use mutsuki_service_runtime::{
    HostEventSource, HostEventSourceContext, HostEventSourceDescriptor, HostEventSourceFuture,
    HostEventSourceHealth,
};
use tokio::sync::{oneshot, watch};

pub struct BilibiliPollingEventSource {
    descriptor: HostEventSourceDescriptor,
    config: SharedBilibiliConfig,
    health: Arc<Mutex<BilibiliPollerHealth>>,
    stop: Arc<Mutex<Option<watch::Sender<bool>>>>,
    stopped: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

#[derive(Clone)]
pub enum BilibiliPollingCredentials {
    WebCookie { secret_key: String, required: bool },
}

impl BilibiliPollingCredentials {
    fn descriptor(&self, descriptor: HostEventSourceDescriptor) -> HostEventSourceDescriptor {
        match self {
            Self::WebCookie {
                secret_key,
                required,
            } if *required => descriptor.require_secret(secret_key.clone()),
            Self::WebCookie { .. } => descriptor,
        }
    }
}

impl BilibiliPollingEventSource {
    #[must_use]
    pub fn new(config: SharedBilibiliConfig, credentials: BilibiliPollingCredentials) -> Self {
        let descriptor = credentials.descriptor(HostEventSourceDescriptor::new(
            "mutsuki.bot.bilibili.polling.source",
            PLUGIN_ID,
        ));
        Self {
            descriptor,
            config,
            health: Arc::new(Mutex::new(BilibiliPollerHealth::default())),
            stop: Arc::new(Mutex::new(None)),
            stopped: Arc::new(Mutex::new(None)),
        }
    }
}

impl HostEventSource for BilibiliPollingEventSource {
    fn descriptor(&self) -> &HostEventSourceDescriptor {
        &self.descriptor
    }

    fn start(&mut self, ctx: HostEventSourceContext) -> HostEventSourceFuture {
        let config = self.config.clone();
        let health = self.health.clone();
        let (stop_tx, stop_rx) = watch::channel(false);
        *self.stop.lock().expect("Bilibili stop mutex") = Some(stop_tx);
        let (stopped_tx, stopped_rx) = oneshot::channel();
        *self.stopped.lock().expect("Bilibili stopped mutex") = Some(stopped_rx);
        Box::pin(async move {
            let result = run_bilibili_polling(
                config,
                health.clone(),
                ctx.task_submitter.clone(),
                stop_rx,
                ctx.shutdown.receiver(),
            )
            .await;
            let _ = stopped_tx.send(());
            result
        })
    }

    fn shutdown(&mut self) -> HostEventSourceFuture {
        let stop = self.stop.lock().expect("Bilibili stop mutex").take();
        let stopped = self.stopped.lock().expect("Bilibili stopped mutex").take();
        Box::pin(async move {
            if let Some(stop) = stop {
                let _ = stop.send(true);
            }
            if let Some(stopped) = stopped {
                let _ = stopped.await;
            }
            Ok(())
        })
    }

    fn health(&self) -> HostEventSourceHealth {
        let health = self.health.lock().expect("Bilibili health mutex").clone();
        if health.running && health.last_error.is_none() {
            HostEventSourceHealth::Healthy
        } else if health.running {
            HostEventSourceHealth::Degraded(health.last_error.unwrap_or_default())
        } else {
            HostEventSourceHealth::Unhealthy(
                health
                    .last_error
                    .unwrap_or_else(|| "polling stopped".into()),
            )
        }
    }
}
