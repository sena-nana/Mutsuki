//! Native plugin ABI surfaces.
//!
//! ABI v1 remains the typed JSONL compatibility bridge. Dispatch and clients
//! consume `mutsuki-runtime-wire` request types; readable method names never
//! escape the debug codec.

mod binary_guest;
mod binary_host_client;
mod dispatch;
mod error;
mod guest;
mod host_client;
mod types;

pub use binary_guest::{BinaryPluginGuest, ConfiguredBinaryPluginGuest, FailedBinaryAbiGuest};
pub use binary_host_client::AbiHostClientV2;
pub use dispatch::{dispatch_binary_host_request, dispatch_host_request};
pub use guest::{ConfiguredJsonlPluginGuest, FailedAbiGuest, JsonlPluginGuest};
pub use host_client::AbiHostClient;
pub use types::{
    ABI_BRIDGE_ID, ABI_CODEC_ID, ABI_ENTRY_SYMBOL, ABI_TRANSPORT_VERSION, ABI_V2_BRIDGE_ID,
    ABI_V2_CODEC_ID, ABI_V2_ENTRY_SYMBOL, ABI_V2_TRANSPORT_VERSION, AbiBuffer, AbiCallResult,
    AbiCloseFn, AbiEntryV1, AbiEntryV2, AbiGuest, AbiHostV1, AbiHostV2, AbiPluginV1, AbiPluginV2,
    AbiReleaseFn, AbiRequestFn, plugin_api_from_guest, plugin_api_v2_from_guest,
};

#[macro_export]
macro_rules! export_mutsuki_plugin_abi_v1 {
    ($factory:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn mutsuki_plugin_abi_v1(
            host: $crate::abi::AbiHostV1,
        ) -> $crate::abi::AbiPluginV1 {
            let host_client = $crate::abi::AbiHostClient::new(host);
            let guest: Box<dyn $crate::abi::AbiGuest> =
                Box::new($crate::abi::ConfiguredJsonlPluginGuest::new(Box::new(
                    move |config| $factory(host_client, config),
                )));
            $crate::abi::plugin_api_from_guest(guest)
        }
    };
}

#[macro_export]
macro_rules! export_mutsuki_plugin_abi_v2 {
    ($factory:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn mutsuki_plugin_abi_v2(
            host: $crate::abi::AbiHostV2,
        ) -> $crate::abi::AbiPluginV2 {
            let host_client = $crate::abi::AbiHostClientV2::new(host);
            let guest: Box<dyn $crate::abi::AbiGuest> =
                Box::new($crate::abi::ConfiguredBinaryPluginGuest::new(Box::new(
                    move |config| $factory(host_client, config),
                )));
            $crate::abi::plugin_api_v2_from_guest(guest)
        }
    };
}

#[cfg(test)]
mod tests;
