//! Standalone-mode Link endpoint descriptors.
//!
//! Embedded and standalone modes share the same WebApplication and protocol.
//! Link transport details stay behind this module; Axum types are never exposed.
//! Control RPC bridging itself lives in `mutsuki-service-link` (ServiceHost).

use std::net::SocketAddr;

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
    /// Resolved socket address for `quic://host:port` endpoints.
    pub quic: Option<SocketAddr>,
}

/// Parse a standalone `link_endpoint` string such as `local://mutsuki.servicehost`
/// or `quic://127.0.0.1:4433`.
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

    match scheme {
        "local" => {
            let link_app = AppId::new(address).map_err(|_| {
                WebHostError::InvalidConfig(format!("invalid link app id in endpoint: {endpoint}"))
            })?;
            let session = SessionIdentity::current();
            let resolved = local_address_for_app(&link_app, &session);
            Ok(LinkBridgeTarget {
                endpoint: endpoint_addr,
                app_id: Some(address.to_string()),
                local: Some(resolved),
                quic: None,
            })
        }
        "quic" => {
            let addr: SocketAddr = address.parse().map_err(|_| {
                WebHostError::InvalidConfig(format!(
                    "invalid quic link endpoint (expected host:port): {endpoint}"
                ))
            })?;
            Ok(LinkBridgeTarget {
                endpoint: endpoint_addr,
                app_id: None,
                local: None,
                quic: Some(addr),
            })
        }
        other => Err(WebHostError::InvalidConfig(format!(
            "unsupported link endpoint scheme `{other}` (supported: local, quic)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_link_endpoint() {
        let target = parse_link_endpoint("local://mutsuki.servicehost").unwrap();
        assert_eq!(target.endpoint.scheme, "local");
        assert_eq!(target.app_id.as_deref(), Some("mutsuki.servicehost"));
        assert!(target.quic.is_none());
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
    fn parses_quic_link_endpoint() {
        let target = parse_link_endpoint("quic://127.0.0.1:4433").unwrap();
        assert_eq!(target.endpoint.scheme, "quic");
        assert_eq!(target.quic.unwrap().to_string(), "127.0.0.1:4433");
        assert!(target.app_id.is_none());
        assert!(target.local.is_none());
    }

    #[test]
    fn rejects_empty_endpoint() {
        assert!(parse_link_endpoint("local://").is_err());
    }

    #[test]
    fn rejects_invalid_local_app_id() {
        assert!(parse_link_endpoint("local://not valid").is_err());
    }

    #[test]
    fn rejects_invalid_quic_address() {
        assert!(parse_link_endpoint("quic://not-a-socket").is_err());
    }
}
