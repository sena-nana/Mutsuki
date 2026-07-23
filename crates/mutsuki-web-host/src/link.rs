//! Standalone-mode Link endpoint descriptors.
//!
//! Embedded and standalone modes share the same WebApplication and protocol.
//! Link transport details stay behind this module; Axum types are never exposed.

use mutsuki_link_core::EndpointAddress;
use mutsuki_link_local::LocalAddress;

use crate::error::{WebHostError, WebHostResult};

/// Parsed standalone bridge target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkBridgeTarget {
    pub endpoint: EndpointAddress,
    pub local: Option<LocalAddress>,
}

/// Parse a standalone `link_endpoint` string such as `local://webhost`.
pub fn parse_link_endpoint(endpoint: &str) -> WebHostResult<LinkBridgeTarget> {
    let (scheme, address) = endpoint
        .split_once("://")
        .ok_or_else(|| WebHostError::InvalidConfig(format!("invalid link endpoint: {endpoint}")))?;

    if scheme.is_empty() || address.is_empty() {
        return Err(WebHostError::InvalidConfig(format!(
            "invalid link endpoint: {endpoint}"
        )));
    }

    let endpoint = EndpointAddress {
        scheme: scheme.to_string(),
        address: address.to_string(),
    };

    let local = if scheme == "local" {
        Some(LocalAddress(address.to_string()))
    } else {
        None
    };

    Ok(LinkBridgeTarget { endpoint, local })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_link_endpoint() {
        let target = parse_link_endpoint("local://webhost").unwrap();
        assert_eq!(target.endpoint.scheme, "local");
        assert_eq!(target.local.unwrap().0, "webhost");
    }

    #[test]
    fn rejects_empty_endpoint() {
        assert!(parse_link_endpoint("local://").is_err());
    }
}
