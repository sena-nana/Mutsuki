//! Native plugin ABI surfaces.
//!
//! ABI v2 uses the length-prefixed typed MessagePack Runtime Wire codec.
//! Dispatch and clients consume `mutsuki-runtime-wire` request types directly.

mod binary_guest;
mod binary_host_client;
mod dispatch;
mod error;
mod guest;
mod types;

pub use binary_guest::{BinaryPluginGuest, ConfiguredBinaryPluginGuest, FailedBinaryAbiGuest};
pub use binary_host_client::AbiHostClient;
pub use dispatch::{dispatch_binary_host_request, dispatch_host_request};
pub use types::{
    ABI_V2_BRIDGE_ID, ABI_V2_CODEC_ID, ABI_V2_ENTRY_SYMBOL, ABI_V2_TRANSPORT_VERSION, AbiBuffer,
    AbiCallResult, AbiCloseFn, AbiEntryV2, AbiGuest, AbiHostV2, AbiPluginV2, AbiReleaseFn,
    AbiRequestFn, plugin_api_from_guest,
};

#[macro_export]
macro_rules! export_mutsuki_plugin_abi_v2 {
    ($factory:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn mutsuki_plugin_abi_v2(
            host: $crate::abi::AbiHostV2,
        ) -> $crate::abi::AbiPluginV2 {
            let host_client = $crate::abi::AbiHostClient::new(host);
            let guest: Box<dyn $crate::abi::AbiGuest> =
                Box::new($crate::abi::ConfiguredBinaryPluginGuest::new(Box::new(
                    move |config| $factory(host_client, config),
                )));
            $crate::abi::plugin_api_from_guest(guest)
        }
    };
}

#[cfg(test)]
mod tests;
