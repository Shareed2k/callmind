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
fn a_pinned_worker_is_accepted_into_the_trust_roots() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server_pem, server_key) = self_signed("callmind.local");
    let (worker_pem, _) = self_signed("gpu-1");

    let server_cert_path = dir.path().join("server.pem");
    let server_key_path = dir.path().join("server-key.pem");
    let worker_cert_path = dir.path().join("gpu-1.pem");
    std::fs::write(&server_cert_path, &server_pem).expect("write");
    std::fs::write(&server_key_path, &server_key).expect("write");
    std::fs::write(&worker_cert_path, &worker_pem).expect("write");

    let tls = WorkerTlsConfig {
        server_cert: server_cert_path,
        server_key: server_key_path,
    };
    let allowed = vec![AllowedWorker {
        name: "gpu-1".to_string(),
        certificate: worker_cert_path,
    }];

    server_tls_config(&tls, &allowed).expect("a pinned worker builds a usable config");
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

#[test]
fn two_pinned_workers_both_build_into_the_trust_roots() {
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

    server_tls_config(&tls, &allowed).expect("two pinned workers build a usable config");
}
