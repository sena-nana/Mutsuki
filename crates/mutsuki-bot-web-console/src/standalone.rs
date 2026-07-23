//! Standalone WebHost + Link control bridge assembly.
//!
//! WebHost validates `link_endpoint` and serves the console shell in
//! [`DeploymentMode::Standalone`]. Control-plane RPC is forwarded to ServiceHost
//! over MutsukiLink local or authenticated QUIC transport.

use std::sync::Arc;

use mutsuki_service_link::{LinkControlHandler, QuicLinkControlHandler, client_config_from_ca_pem};
use mutsuki_web_host::{MutsukiWebHost, WebHostResult, parse_link_endpoint};
use mutsuki_web_protocol::DeploymentMode;

use crate::{
    ConsoleAssetDirs, WebConsolePaths, WebConsoleSecrets, base_builder, materialize_console_shell,
};

/// Caller-resolved QUIC TLS trust material for `quic://` Link endpoints.
///
/// Product config only stores secret key references; assembly resolves PEM values
/// before constructing this struct.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StandaloneQuicTlsIdentity {
    /// TLS server name (SNI) expected by the QUIC peer.
    pub server_name: String,
    /// PEM-encoded CA / server certificate(s) the client should trust.
    pub ca_cert_pem: String,
}

/// Standalone console build inputs (Link endpoint required).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StandaloneConsoleSpec {
    pub listen: String,
    pub link_endpoint: String,
    pub auth_token: String,
    pub include_config: bool,
    pub include_upgrade: bool,
    /// Required when `link_endpoint` uses `quic://`.
    pub quic_tls: Option<StandaloneQuicTlsIdentity>,
}

/// Build a standalone WebHost console shell with Link-bridged control RPC.
pub fn build_standalone_console_host(
    spec: &StandaloneConsoleSpec,
    paths: &WebConsolePaths,
) -> WebHostResult<(MutsukiWebHost, ConsoleAssetDirs)> {
    if spec.link_endpoint.trim().is_empty() {
        return Err(mutsuki_web_host::WebHostError::InvalidConfig(
            "standalone console requires non-empty link_endpoint".into(),
        ));
    }

    let target = parse_link_endpoint(&spec.link_endpoint)?;
    let control: Arc<dyn mutsuki_service_control::ControlHandler> = if let Some(app_id) =
        target.app_id
    {
        Arc::new(LinkControlHandler::for_app(app_id))
    } else if let Some(addr) = target.quic {
        let identity = spec.quic_tls.as_ref().ok_or_else(|| {
                mutsuki_web_host::WebHostError::InvalidConfig(
                    "standalone quic:// bridge requires quic_tls (server_name + ca_cert_pem secret material)".into(),
                )
            })?;
        if identity.server_name.trim().is_empty() {
            return Err(mutsuki_web_host::WebHostError::InvalidConfig(
                "standalone quic:// bridge requires non-empty quic_tls.server_name".into(),
            ));
        }
        if identity.ca_cert_pem.trim().is_empty() {
            return Err(mutsuki_web_host::WebHostError::InvalidConfig(
                "standalone quic:// bridge requires non-empty quic_tls.ca_cert_pem".into(),
            ));
        }
        let client_config = client_config_from_ca_pem(&identity.ca_cert_pem).map_err(|error| {
            mutsuki_web_host::WebHostError::InvalidConfig(format!(
                "standalone quic:// TLS identity invalid: {error}"
            ))
        })?;
        Arc::new(QuicLinkControlHandler::new(
            addr,
            identity.server_name.clone(),
            client_config,
        ))
    } else {
        return Err(mutsuki_web_host::WebHostError::InvalidConfig(
            "standalone control bridge requires local:// or quic:// link endpoint".into(),
        ));
    };
    let secrets = WebConsoleSecrets {
        auth_token: spec.auth_token.clone(),
    };
    let config = crate::WebConsoleConfig {
        enabled: true,
        listen: spec.listen.clone(),
        auth_token_key: None,
        include_config: spec.include_config,
        release_set: paths
            .release_set
            .as_ref()
            .and_then(|path| path.to_str().map(str::to_string)),
        link_endpoint: Some(spec.link_endpoint.clone()),
        quic_server_name: spec.quic_tls.as_ref().map(|tls| tls.server_name.clone()),
        quic_ca_cert_key: None,
    };
    let asset_dirs = ConsoleAssetDirs::materialize(spec.include_config, spec.include_upgrade)?;
    materialize_console_shell(
        &asset_dirs.overview_assets,
        spec.include_config,
        spec.include_upgrade,
    )
    .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;

    let caller =
        mutsuki_plugin_bot_control_web::ControlRpcCaller::new(control, spec.auth_token.clone());
    let mut builder = base_builder(&config, &secrets, &asset_dirs)
        .mode(DeploymentMode::Standalone)
        .link_endpoint(&spec.link_endpoint);
    builder = builder.extension(mutsuki_plugin_bot_control_web::ControlWebExtension::new(
        caller.clone(),
    ));
    builder = builder.extension(
        mutsuki_plugin_bot_overview_web::OverviewWebExtension::new(caller)
            .with_frontend_assets(&asset_dirs.overview_assets),
    );
    Ok((builder.build()?, asset_dirs))
}
