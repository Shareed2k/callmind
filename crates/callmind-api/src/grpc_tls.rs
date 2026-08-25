//! Mutual TLS for the worker listener.
//!
//! Workers are pinned to their certificates rather than issued by a certificate
//! authority. A self-signed certificate is its own root, so pinning is done by
//! handing each worker's certificate to the server as a trust root: rustls then
//! accepts exactly those clients and no others.

use callmind_config::{AllowedWorker, WorkerTlsConfig};
use std::path::PathBuf;
use tonic::transport::{Certificate, Identity, ServerTlsConfig};

/// Why a TLS configuration could not be built.
#[derive(Debug, thiserror::Error)]
pub enum TlsSetupError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no workers are pinned, so nobody could connect")]
    NoWorkersPinned,
}

/// Build the listener's TLS configuration from the server's own certificate and
/// the pinned worker certificates.
pub fn server_tls_config(
    tls: &WorkerTlsConfig,
    allowed: &[AllowedWorker],
) -> Result<ServerTlsConfig, TlsSetupError> {
    if allowed.is_empty() {
        return Err(TlsSetupError::NoWorkersPinned);
    }

    let cert = read(&tls.server_cert)?;
    let key = read(&tls.server_key)?;

    // One PEM bundle of every pinned certificate: rustls reads them all and
    // will accept a client presenting any one of them.
    let mut roots = Vec::new();
    for worker in allowed {
        roots.extend_from_slice(&read(&worker.certificate)?);
        roots.push(b'\n');
    }

    Ok(ServerTlsConfig::new()
        .identity(Identity::from_pem(cert, key))
        .client_ca_root(Certificate::from_pem(roots)))
}

fn read(path: &PathBuf) -> Result<Vec<u8>, TlsSetupError> {
    std::fs::read(path).map_err(|source| TlsSetupError::Read {
        path: path.clone(),
        source,
    })
}
