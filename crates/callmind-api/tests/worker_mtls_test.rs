//! Mutual TLS on the worker listener, over a real socket.
//!
//! A worker leases jobs and downloads recordings, so the listener has to prove
//! who is calling before it hands anything over.

use callmind_api::grpc_tls::server_tls_config;
use callmind_config::{AllowedWorker, WorkerTlsConfig};

/// A self-signed certificate and its key, as PEM.
fn self_signed(name: &str) -> (String, String) {
    let cert =
        rcgen::generate_simple_self_signed(vec![name.to_string()]).expect("generate a certificate");
    (cert.cert.pem(), cert.signing_key.serialize_pem())
}

#[test]
fn a_missing_certificate_file_is_reported_with_its_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server_pem, server_key) = self_signed("callmind.local");
    let server_cert_path = dir.path().join("server.pem");
    let server_key_path = dir.path().join("server-key.pem");
    std::fs::write(&server_cert_path, &server_pem).expect("write");
    std::fs::write(&server_key_path, &server_key).expect("write");

    let tls = WorkerTlsConfig {
        server_cert: server_cert_path,
        server_key: server_key_path,
    };
    let allowed = vec![AllowedWorker {
        name: "gpu-1".to_string(),
        certificate: dir.path().join("absent.pem"),
    }];

    let err = server_tls_config(&tls, &allowed).expect_err("the file is not there");
    assert!(
        format!("{err}").contains("absent.pem"),
        "the error names the file: {err}"
    );
}

/// Pins two workers instead of one and checks that `server_tls_config` still
/// succeeds: it reads both PEM files and concatenates them without error.
///
/// On its own this proves nothing about rustls: `Certificate::from_pem` and
/// `Identity::from_pem` just wrap raw bytes, and tonic only parses the PEM into
/// a `RootCertStore` inside `Server::tls_config()`, which this test never
/// reaches. That the bundle is genuinely accepted -- every worker in it, not
/// just the first -- is settled over a real socket by
/// `every_worker_in_the_pinned_bundle_can_connect_and_is_told_apart` in
/// `worker_grpc_test.rs`.
#[test]
fn two_pinned_workers_are_read_and_concatenated_without_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server_pem, server_key) = self_signed("callmind.local");
    let (worker_a_pem, _) = self_signed("gpu-1");
    let (worker_b_pem, _) = self_signed("gpu-2");

    let server_cert_path = dir.path().join("server.pem");
    let server_key_path = dir.path().join("server-key.pem");
    let worker_a_path = dir.path().join("gpu-1.pem");
    let worker_b_path = dir.path().join("gpu-2.pem");
    std::fs::write(&server_cert_path, &server_pem).expect("write");
    std::fs::write(&server_key_path, &server_key).expect("write");
    std::fs::write(&worker_a_path, &worker_a_pem).expect("write");
    std::fs::write(&worker_b_path, &worker_b_pem).expect("write");

    let tls = WorkerTlsConfig {
        server_cert: server_cert_path,
        server_key: server_key_path,
    };
    let allowed = vec![
        AllowedWorker {
            name: "gpu-1".to_string(),
            certificate: worker_a_path,
        },
        AllowedWorker {
            name: "gpu-2".to_string(),
            certificate: worker_b_path,
        },
    ];

    server_tls_config(&tls, &allowed)
        .expect("reading and concatenating two workers' certs still builds a config");
}
