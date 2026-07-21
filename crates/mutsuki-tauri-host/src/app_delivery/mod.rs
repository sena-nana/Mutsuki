//! Cross-app delivery: activate target Tauri apps, wait until Link/capability ready,
//! then transmit typed capability requests with idempotent receipts.
//!
//! Business apps call [`AppDeliveryService::request_app`] instead of launching
//! processes, polling sockets, or stuffing payloads into argv.

mod activator;
mod delivery;
mod draft;
mod endpoint;
mod transport;
mod types;

#[cfg(test)]
mod tests;

pub use activator::{NullAppActivator, ProcessAppActivator, TauriAppActivator};
pub use delivery::AppDeliveryService;
pub use draft::{DeliveryDraft, DeliveryDraftStore};
pub use endpoint::AppCapabilityEndpoint;
pub use transport::{
    AppLinkSession, AppLinkTransport, InMemoryAppLinkTransport, LinkLocalAppTransport,
};
pub use types::{
    ActivationError, ActivationReceipt, AppDeliveryError, AppDeliveryOptions, AppDescriptor, AppId,
    AppIdentity, DeliveryPhase, HOST_PROTOCOL_VERSION,
};
