//! Standalone WebHost + Link endpoint assembly skeleton.
//!
//! WebHost validates `link_endpoint` and serves the console shell in
//! [`DeploymentMode::Standalone`]. Control-plane RPC still requires an in-process
//! [`ControlHandler`]; Link transport that proxies control over MutsukiLink is
//! **not wired yet**. Until the WebHost Link bridge lands, use
//! [`UnwiredLinkControlHandler`] so `control.*` fails with structured
//! [`STANDALONE_LINK_NOT_WIRED`] instead of returning fixture/demo data.

use std::sync::Arc;

use mutsuki_service_control::{
    ControlError, ControlFuture, ControlHandler, ControlRequest, ControlResponse,
};
use mutsuki_web_host::{MutsukiWebHost, WebHostResult};
use mutsuki_web_protocol::DeploymentMode;

use crate::{
    ConsoleAssetDirs, WebConsolePaths, WebConsoleSecrets, base_builder, materialize_console_shell,
};

pub const STANDALONE_LINK_NOT_WIRED: &str = "standalone.link_not_wired";

/// Standalone console build inputs (Link endpoint required).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StandaloneConsoleSpec {
    pub listen: String,
    pub link_endpoint: String,
    pub auth_token: String,
    pub include_config: bool,
    pub include_upgrade: bool,
}

/// Placeholder control plane for standalone builds before Link transport ships.
#[derive(Clone, Default)]
pub struct UnwiredLinkControlHandler;

impl ControlHandler for UnwiredLinkControlHandler {
    fn handle(&self, _request: ControlRequest) -> ControlFuture {
        Box::pin(async move {
            ControlResponse::err(ControlError::Failed(format!(
                "{STANDALONE_LINK_NOT_WIRED}: Link transport not wired; \
                 embedded console or WebHost Link bridge required"
            )))
        })
    }
}

/// Build a standalone WebHost console shell. Control RPC fails until Link bridge is wired.
pub fn build_standalone_console_host(
    spec: &StandaloneConsoleSpec,
    paths: &WebConsolePaths,
) -> WebHostResult<(MutsukiWebHost, ConsoleAssetDirs)> {
    if spec.link_endpoint.trim().is_empty() {
        return Err(mutsuki_web_host::WebHostError::InvalidConfig(
            "standalone console requires non-empty link_endpoint".into(),
        ));
    }

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
    };
    let asset_dirs = ConsoleAssetDirs::materialize(spec.include_config, spec.include_upgrade)?;
    materialize_console_shell(
        &asset_dirs.overview_assets,
        spec.include_config,
        spec.include_upgrade,
    )
    .map_err(|err| mutsuki_web_host::WebHostError::Io(err.to_string()))?;

    let caller = mutsuki_plugin_bot_control_web::ControlRpcCaller::new(
        Arc::new(UnwiredLinkControlHandler),
        spec.auth_token.clone(),
    );
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
