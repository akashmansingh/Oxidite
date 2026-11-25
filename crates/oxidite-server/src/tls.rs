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
pub fn load_tls_config_from_pem(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> Result<Arc<ServerConfig>, TlsError> {
    use rustls_pki_types::pem::{Error as PemError, PemObject};

    let certs = CertificateDer::pem_file_iter(cert_path)
        .map_err(|e| match e {
            PemError::Io(io) => TlsError::Io(io),
            other => TlsError::MissingCert(format!("{}: {other}", cert_path.display())),
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| match e {
            PemError::Io(io) => TlsError::Io(io),
            other => TlsError::MissingCert(format!("{}: {other}", cert_path.display())),
        })?;
    if certs.is_empty() {
        return Err(TlsError::MissingCert(cert_path.display().to_string()));
    }

    let key = PrivateKeyDer::from_pem_file(key_path).map_err(|e| match e {
        PemError::Io(io) => TlsError::Io(io),
        other => TlsError::MissingKey(format!("{}: {other}", key_path.display())),
    })?;

    build_server_config(certs, key)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn test_requires_tls_loopback_vs_non_loopback() {
        let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3030);
        let wildcard = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 3030);
        let lan = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 8443);

        assert!(!requires_tls(&loopback));
        assert!(requires_tls(&wildcard));
        assert!(requires_tls(&lan));
    }

    #[test]
    fn test_load_tls_config_from_pem_rejects_missing_files() {
        let err = load_tls_config_from_pem(
            std::path::Path::new("nonexistent-cert.pem"),
            std::path::Path::new("nonexistent-key.pem"),
        )
        .unwrap_err();
        assert!(matches!(err, TlsError::Io(_) | TlsError::MissingCert(_)));
    }
}
