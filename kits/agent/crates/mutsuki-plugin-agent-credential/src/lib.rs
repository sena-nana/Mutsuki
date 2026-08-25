// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::unused_async,
    clippy::wildcard_imports
)]

pub mod plugin;

pub use plugin::*;

use std::sync::Arc;

use mutsuki_agent_adapter_api::{CredentialBroker, CredentialFuture, CredentialValue};
use mutsuki_agent_contracts::{
    CREDENTIAL_UNAVAILABLE, CredentialRef, ProtocolError, ProtocolErrorClass,
};
use mutsuki_agent_runtime::CredentialBrokerService;

/// Bridges runtime CredentialBrokerService into the adapter-facing CredentialBroker trait.
#[derive(Clone)]
pub struct AdapterCredentialBroker {
    service: CredentialBrokerService,
}

impl AdapterCredentialBroker {
    pub fn new(service: CredentialBrokerService) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &CredentialBrokerService {
        &self.service
    }
}

impl CredentialBroker for AdapterCredentialBroker {
    fn resolve(&self, credential: CredentialRef) -> CredentialFuture {
        let service = self.service.clone();
        Box::pin(async move {
            match service.resolve_secret(&credential) {
                Ok(secret) => CredentialValue::new(secret).map_err(|error| ProtocolError {
                    code: error.code.clone(),
                    class: ProtocolErrorClass::Authentication,
                    message: error.message,
                    retry_after_ms: None,
                }),
                Err(error) => Err(ProtocolError {
                    code: if error.code.is_empty() {
                        CREDENTIAL_UNAVAILABLE.into()
                    } else {
                        error.code
                    },
                    class: ProtocolErrorClass::Authentication,
                    message: error.message,
                    retry_after_ms: None,
                }),
            }
        })
    }
}

pub fn adapter_broker(service: CredentialBrokerService) -> Arc<dyn CredentialBroker> {
    Arc::new(AdapterCredentialBroker::new(service))
}
