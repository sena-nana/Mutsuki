//! Standalone-mode Link endpoint descriptors.
//!
//! Embedded and standalone modes share the same WebApplication and protocol.
//! Link transport details stay behind this module; Axum types are never exposed.
//! Control RPC bridging itself lives in `mutsuki-service-link` (ServiceHost).

use mutsuki_link_core::EndpointAddress;
use mutsuki_link_local::{AppId, LocalAddress, SessionIdentity, local_address_for_app};

use crate::error::{WebHostError, WebHostResult};

/// Parsed standalone bridge target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkBridgeTarget {
    pub endpoint: EndpointAddress,
    /// App id for `local://` endpoints (e.g. `mutsuki.servicehost`).
    pub app_id: Option<String>,
    pub local: Option<LocalAddress>,
}

/// Parse a standalone `link_endpoint` string such as `local://mutsuki.servicehost`.
pub fn parse_link_endpoint(endpoint: &str) -> WebHostResult<LinkBridgeTarget> {
    let (scheme, address) = endpoint
        .split_once("://")
        .ok_or_else(|| WebHostError::InvalidConfig(format!("invalid link endpoint: {endpoint}")))?;

    if scheme.is_empty() || address.is_empty() {
        return Err(WebHostError::InvalidConfig(format!(
            "invalid link endpoint: {endpoint}"
        )));
    }

    let endpoint_addr = EndpointAddress {
        scheme: scheme.to_string(),
        address: address.to_string(),
    };

    let (app_id, local) = if scheme == "local" {
        let link_app = AppId::new(address).map_err(|_| {
            WebHostError::InvalidConfig(format!("invalid link app id in endpoint: {endpoint}"))
        })?;
        let session = SessionIdentity::current();
        let resolved = local_address_for_app(&link_app, &session);
        (Some(address.to_string()), Some(resolved))
    } else {
        (None, None)
    };

    Ok(LinkBridgeTarget {
        endpoint: endpoint_addr,
        app_id,
        local,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_link_endpoint() {
        let target = parse_link_endpoint("local://mutsuki.servicehost").unwrap();
        assert_eq!(target.endpoint.scheme, "local");
        assert_eq!(target.app_id.as_deref(), Some("mutsuki.servicehost"));
        assert!(
            target
                .local
                .as_ref()
                .unwrap()
                .0
                .starts_with("mutsuki.app.mutsuki-servicehost.")
        );
    }

    #[test]
    fn rejects_empty_endpoint() {
        assert!(parse_link_endpoint("local://").is_err());
    }

    #[test]
    fn rejects_invalid_local_app_id() {
        assert!(parse_link_endpoint("local://not valid").is_err());
    }
}
