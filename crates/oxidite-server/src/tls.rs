//! TLS configuration for oxidited using rustls.

use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::sync::Arc;
use tokio_rustls::rustls::ServerConfig;

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TLS configuration error: {0}")]
    Config(#[from] tokio_rustls::rustls::Error),
    #[error("No certificate found in {0}")]
    MissingCert(String),
    #[error("No private key found in {0}")]
    MissingKey(String),
}

/// Builds a rustls `ServerConfig` given in-memory DER certificates and private key.
pub fn build_server_config(
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<ServerConfig>, TlsError> {
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// Checks whether an address requires TLS under the Oxidite security model.
/// Non-loopback binds require TLS.
pub fn requires_tls(addr: &std::net::SocketAddr) -> bool {
    !addr.ip().is_loopback()
}
