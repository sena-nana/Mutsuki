/// Installs the workspace-selected rustls provider before an HTTP client is built.
///
/// This is idempotent and preserves a provider already selected by the host.
pub fn ensure_http_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
