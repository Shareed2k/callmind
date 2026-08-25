//! The worker gRPC contract, exercised over a real socket.
//!
//! This is the plugin boundary, so it is tested through the wire rather than by
//! calling the service methods directly: a third party's worker sees exactly
//! this, including the status codes it has to react to.

use callmind_analysis::AnalysisEngine;
use callmind_api::grpc::WorkerService;
use callmind_api::grpc_tls::{pinned_worker_names, server_tls_config};
use callmind_api::state::AppState;
use callmind_config::{AllowedWorker, AppConfig, WorkerTlsConfig};
use callmind_core::{
    Call, CallDirection, CallId, EnqueueJob, JobKind, JobStatus, OrgId, ProcessingStatus, Recording,
};
use callmind_db::{
    AnalysisRow, CallRepository, JobRepository, SqlCallRepository, SqlJobRepository,
    SqlSearchIndex, SqlStatsRepository, create_sqlite_pool, orm_connection, run_migrations,
};
use callmind_llm::MockLlmEngine;
use callmind_search::{AskEngine, SearchEngine};
use callmind_storage::{FilesystemStorage, RecordingStorage};
use callmind_worker_proto::v1::worker_client::WorkerClient;
use callmind_worker_proto::v1::worker_server::WorkerServer;
use callmind_worker_proto::{convert, v1};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::Code;
use tonic::transport::Channel;

/// The audio a worker will stream back.
const AUDIO: &[u8] = b"not really audio, but the bytes have to arrive intact";

struct Fixture {
    client: WorkerClient<Channel>,
    call_repo: Arc<SqlCallRepository>,
    job_repo: Arc<SqlJobRepository>,
    call_id: CallId,
    _dir: tempfile::TempDir,
}

/// Everything a fixture needs except the listener, so the plain and the TLS
/// listener are served the same database, recording and queued job.
struct Harness {
    state: AppState,
    call_repo: Arc<SqlCallRepository>,
    job_repo: Arc<SqlJobRepository>,
    call_id: CallId,
    dir: tempfile::TempDir,
}

async fn harness() -> Harness {
    let pool = create_sqlite_pool(":memory:", 5, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(FilesystemStorage::new(dir.path()).await.unwrap());

    let call_repo = Arc::new(SqlCallRepository::new(orm_connection(&pool)));
    let job_repo = Arc::new(SqlJobRepository::new(orm_connection(&pool)));
    let stats_repo = Arc::new(SqlStatsRepository::new(orm_connection(&pool)));

    let call = Call::new(
        OrgId::DEFAULT,
        Some("grpc-test".into()),
        CallDirection::Incoming,
        None,
        None,
        None,
    );
    call_repo.create(&call).await.unwrap();

    let storage_key = format!("{}/{}.m4a", OrgId::DEFAULT, call.id);
    let stream = Box::pin(futures_util::stream::once(async move {
        Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from_static(AUDIO))
    }));
    let put = storage.put(&storage_key, stream).await.unwrap();
    call_repo
        .add_recording(&Recording::new(
            call.id,
            storage_key,
            "audio/mp4".into(),
            put.size_bytes,
            put.sha256,
        ))
        .await
        .unwrap();

    job_repo
        .enqueue(
            &EnqueueJob::new(
                JobKind::IngestRecording,
                serde_json::json!({ "call_id": call.id.to_string(), "language_hint": "he" }),
            )
            .with_call_id(call.id),
        )
        .await
        .unwrap();

    let search = Arc::new(SearchEngine::new(Arc::new(SqlSearchIndex::new(
        orm_connection(&pool),
    ))));
    let llm = Arc::new(MockLlmEngine::default());
    let state = AppState::new(
        Arc::new(AppConfig::default()),
        call_repo.clone(),
        call_repo.clone(),
        job_repo.clone(),
        stats_repo.clone(),
        storage,
        search.clone(),
        Arc::new(AskEngine::new((*search).clone(), llm.clone())),
        Arc::new(AnalysisEngine::new(llm)),
        Arc::new(callmind_ui::templates::TemplateRegistry::new()),
    );

    Harness {
        state,
        call_repo,
        job_repo,
        call_id: call.id,
        dir,
    }
}

/// Serve `state` over plain TCP, the way the loopback-only listener runs, and
/// connect a client presenting no certificate at all.
async fn serve_plain(state: AppState) -> WorkerClient<Channel> {
    // Ephemeral port so tests can run in parallel.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(WorkerServer::new(WorkerService::new(state)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    loop {
        match WorkerClient::connect(format!("http://{addr}")).await {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
}

async fn start() -> Fixture {
    let h = harness().await;
    let client = serve_plain(h.state.clone()).await;

    Fixture {
        client,
        call_repo: h.call_repo,
        job_repo: h.job_repo,
        call_id: h.call_id,
        _dir: h.dir,
    }
}

/// Queue another job for the fixture's call, so a test can make explicit that
/// it needs one leasable rather than relying on the one `harness` already
/// queued.
async fn enqueue_job(f: &Fixture) {
    f.job_repo
        .enqueue(
            &EnqueueJob::new(
                JobKind::IngestRecording,
                serde_json::json!({ "call_id": f.call_id.to_string() }),
            )
            .with_call_id(f.call_id),
        )
        .await
        .unwrap();
}

/// A self-signed certificate and its key, as PEM. A worker is pinned to exactly
/// this certificate, so it is its own root.
fn self_signed(name: &str) -> (String, String) {
    let cert =
        rcgen::generate_simple_self_signed(vec![name.to_string()]).expect("generate a certificate");
    (cert.cert.pem(), cert.signing_key.serialize_pem())
}

/// The same service behind mutual TLS, with one certificate per named worker.
struct TlsFixture {
    addr: std::net::SocketAddr,
    /// The server's own certificate, which a client trusts as its root.
    ca_pem: String,
    client_certs: HashMap<String, (String, String)>,
    /// Held so the storage directory and the queued job outlive the server.
    _harness: Harness,
    _certs: tempfile::TempDir,
}

impl TlsFixture {
    /// A client presenting `name`'s certificate.
    async fn client_as(&self, name: &str) -> WorkerClient<Channel> {
        let (cert, key) = self.client_certs.get(name).expect("known worker").clone();
        self.connect_with(cert, key)
            .await
            .expect("a pinned certificate connects")
    }

    /// Try to connect presenting any certificate at all, pinned or not.
    async fn connect_with(
        &self,
        cert: String,
        key: String,
    ) -> Result<WorkerClient<Channel>, tonic::transport::Error> {
        let tls = tonic::transport::ClientTlsConfig::new()
            .domain_name("callmind.local")
            .ca_certificate(tonic::transport::Certificate::from_pem(self.ca_pem.clone()))
            .identity(tonic::transport::Identity::from_pem(cert, key));
        let endpoint = Channel::from_shared(format!("https://{}", self.addr))
            .unwrap()
            .tls_config(tls)
            .unwrap();

        // Retried but bounded: the server task may not have reached `accept`
        // yet, while a refused handshake would fail forever.
        let mut attempt = endpoint.connect().await;
        for _ in 0..10 {
            if attempt.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            attempt = endpoint.connect().await;
        }
        attempt.map(WorkerClient::new)
    }
}

/// Serve the worker contract over mutual TLS, pinning one certificate per name.
async fn start_with_tls(workers: &[&str]) -> TlsFixture {
    let certs = tempfile::tempdir().unwrap();
    let (server_pem, server_key) = self_signed("callmind.local");
    let server_cert_path = certs.path().join("server.pem");
    let server_key_path = certs.path().join("server-key.pem");
    std::fs::write(&server_cert_path, &server_pem).unwrap();
    std::fs::write(&server_key_path, &server_key).unwrap();

    let mut client_certs = HashMap::new();
    let mut allowed = Vec::new();
    for name in workers {
        let (pem, key) = self_signed(name);
        let path = certs.path().join(format!("{name}.pem"));
        std::fs::write(&path, &pem).unwrap();
        allowed.push(AllowedWorker {
            name: (*name).to_string(),
            certificate: path,
        });
        client_certs.insert((*name).to_string(), (pem, key));
    }

    let tls = WorkerTlsConfig {
        server_cert: server_cert_path,
        server_key: server_key_path,
    };
    // The production paths, not a test-only imitation: the same builder the
    // server uses, and the same fingerprint map startup builds.
    let tls_config = server_tls_config(&tls, &allowed).expect("a TLS configuration");
    let worker_names = pinned_worker_names(&allowed).expect("a fingerprint per worker");

    let h = harness().await;
    let state = h.state.clone().with_worker_names(worker_names);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls_config)
            .expect("apply TLS")
            .add_service(WorkerServer::new(WorkerService::new(state)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    TlsFixture {
        addr,
        ca_pem: server_pem,
        client_certs,
        _harness: h,
        _certs: certs,
    }
}

fn sample_transcript(call_id: CallId) -> v1::Transcript {
    convert::transcript_to_proto(&callmind_transcript::Transcript {
        call_id,
        languages: vec![],
        speakers: vec![],
        segments: vec![callmind_transcript::TranscriptSegment {
            id: uuid::Uuid::new_v4(),
            call_id,
            sequence: 0,
            speaker_id: callmind_core::SpeakerId(1),
            speaker_role: callmind_core::SpeakerRole::Customer,
            language: callmind_core::Language::Hebrew,
            text_direction: callmind_transcript::TextDirection::Rtl,
            start_ms: 0,
            end_ms: 900,
            raw_text: "שלום".into(),
            normalized_text: "שלום".into(),
            words: vec![],
        }],
    })
}

#[tokio::test]
async fn a_worker_can_lease_stream_and_submit() {
    let mut f = start().await;

    // 1. Lease.
    let lease = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "gpu-box-1".into(),
            kinds: vec!["ingest_recording".into()],
        })
        .await
        .unwrap()
        .into_inner();
    let job = lease.job.expect("a job should be available");
    assert_eq!(job.call_id, f.call_id.to_string());
    assert_eq!(job.mime_type, "audio/mp4");
    assert_eq!(job.file_size_bytes, AUDIO.len() as u64);
    // A pinned language is surfaced as its own field, not left in the payload.
    assert_eq!(job.language_hint, "he");
    assert!(lease.lease_timeout_secs > 0);

    // 2. A second worker must not get the same job.
    let second = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "gpu-box-2".into(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner();
    assert!(second.job.is_none(), "job leased twice");

    // 3. Streamed audio must arrive byte-for-byte.
    let mut stream = f
        .client
        .stream_recording(v1::StreamRecordingRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();

    let mut received = Vec::new();
    while let Some(chunk) = stream.message().await.unwrap() {
        received.extend_from_slice(&chunk.data);
    }
    assert_eq!(received, AUDIO);

    // 4. Heartbeat reports lease ownership rather than erroring.
    let held = f
        .client
        .heartbeat(v1::HeartbeatRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(held.still_leased);

    let not_held = f
        .client
        .heartbeat(v1::HeartbeatRequest {
            worker_id: "gpu-box-2".into(),
            job_id: job.job_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(
        !not_held.still_leased,
        "a non-holder must be told it lost the lease"
    );

    // 5. Only the lease holder may submit.
    let err = f
        .client
        .submit_transcript(v1::SubmitTranscriptRequest {
            worker_id: "gpu-box-2".into(),
            job_id: job.job_id.clone(),
            transcript: Some(sample_transcript(f.call_id)),
        })
        .await
        .expect_err("a non-holder must be refused");
    assert_eq!(err.code(), Code::Aborted);

    // 6. A malformed transcript is refused up front.
    let err = f
        .client
        .submit_transcript(v1::SubmitTranscriptRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
            transcript: Some(v1::Transcript {
                call_id: f.call_id.to_string(),
                languages: vec![],
                speakers: vec![],
                segments: vec![],
            }),
        })
        .await
        .expect_err("an empty transcript must be refused");
    assert_eq!(err.code(), Code::InvalidArgument);

    // 7. The real submission stores the transcript and requeues for analysis.
    let stored = f
        .client
        .submit_transcript(v1::SubmitTranscriptRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
            transcript: Some(sample_transcript(f.call_id)),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(stored.segments_stored, 1);

    let saved = f
        .call_repo
        .get_transcript_json(f.call_id)
        .await
        .unwrap()
        .expect("transcript stored");
    assert!(saved.contains("שלום"));

    let requeued = f
        .job_repo
        .get_by_id(job.job_id.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        requeued.status,
        JobStatus::Pending,
        "job should return to the queue for the analysis stage"
    );
}

#[tokio::test]
async fn a_worker_can_submit_a_plugin_result() {
    let mut f = start().await;
    // A plugin job, not the pipeline job the fixture enqueues: a plugin result
    // finishes the job its lease covers, so it may only be sent against a job
    // that has nothing else to do afterwards.
    f.job_repo
        .enqueue(
            &EnqueueJob::new(
                JobKind::Custom("acoustic-emotions".into()),
                serde_json::json!({ "call_id": f.call_id.to_string() }),
            )
            .with_call_id(f.call_id),
        )
        .await
        .unwrap();

    let job = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "gpu-box-1".into(),
            kinds: vec!["plugin:acoustic-emotions".into()],
        })
        .await
        .unwrap()
        .into_inner()
        .job
        .unwrap();

    // The refusals come first: an accepted result finishes the job, and a
    // finished job has no lease left to submit under.

    // A plugin name that could escape a storage key or a template path.
    let err = f
        .client
        .submit_plugin_result(v1::SubmitPluginResultRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
            plugin: "../escape".into(),
            payload: Some(v1::submit_plugin_result_request::Payload::Json("{}".into())),
        })
        .await
        .expect_err("plugin names must be restricted");
    assert_eq!(err.code(), Code::InvalidArgument);

    // The JSON escape hatch must actually be JSON.
    let err = f
        .client
        .submit_plugin_result(v1::SubmitPluginResultRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
            plugin: "custom".into(),
            payload: Some(v1::submit_plugin_result_request::Payload::Json(
                "not json".into(),
            )),
        })
        .await
        .expect_err("non-JSON payloads must be refused");
    assert_eq!(err.code(), Code::InvalidArgument);

    f.client
        .submit_plugin_result(v1::SubmitPluginResultRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id,
            plugin: "acoustic-emotions".into(),
            payload: Some(v1::submit_plugin_result_request::Payload::SpeakerEmotions(
                v1::SpeakerEmotions {
                    call_id: f.call_id.to_string(),
                    model: "wav2vec2-emotion".into(),
                    summaries: vec![v1::SpeakerEmotionSummary {
                        speaker_id: 1,
                        dominant: "joy".into(),
                        scores: vec![v1::EmotionScore {
                            emotion: "joy".into(),
                            score: 0.8,
                        }],
                    }],
                    spans: vec![],
                },
            )),
        })
        .await
        .unwrap();

    let results = f.call_repo.list_plugin_results(f.call_id).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "acoustic-emotions");
    assert!(results[0].1.contains("wav2vec2-emotion"));
    assert!(results[0].1.contains("\"dominant\":\"joy\""));
}

#[tokio::test]
async fn a_worker_can_give_a_job_back() {
    let mut f = start().await;
    let job = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "gpu-box-1".into(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner()
        .job
        .unwrap();

    f.client
        .fail_job(v1::FailJobRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id,
            error: "CUDA out of memory".into(),
            retryable: false,
        })
        .await
        .unwrap();

    // A non-retryable failure has to surface on the call, not just the job.
    let call = f.call_repo.get_by_id(f.call_id).await.unwrap().unwrap();
    assert_eq!(call.processing_status, ProcessingStatus::Failed);
}

/// `worker_id` is a string the caller chooses, so it cannot decide what the
/// caller may touch. Temporal treats worker identity the same way -- a label,
/// with authorization coming from the certificate. Without TLS there is no
/// certificate, so the service falls back to the declared id and says so.
///
/// `start()` has already enqueued a job, so there is one to lease.
#[tokio::test]
async fn without_tls_the_declared_worker_id_is_used_and_still_scopes_the_lease() {
    let mut f = start().await;

    let leased = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "worker-a".into(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner()
        .job
        .expect("a job");

    // A second worker claiming the first one's job is refused.
    let err = f
        .client
        .submit_plugin_result(v1::SubmitPluginResultRequest {
            worker_id: "worker-b".into(),
            job_id: leased.job_id.clone(),
            plugin: "acoustic-emotions".into(),
            payload: Some(v1::submit_plugin_result_request::Payload::Json("{}".into())),
        })
        .await
        .expect_err("worker-b holds no lease");

    assert_eq!(err.code(), Code::Aborted, "{err:?}");
}

/// With TLS configured, the certificate decides. A caller presenting the
/// certificate pinned to `gpu-1` cannot act as `gpu-2` by saying so.
#[tokio::test]
async fn with_tls_the_certificate_decides_not_the_declared_id() {
    let f = start_with_tls(&["gpu-1"]).await;

    let mut client = f.client_as("gpu-1").await;
    let leased = client
        .lease(v1::LeaseRequest {
            // Deliberately a different string from the certificate name.
            worker_id: "gpu-2".into(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner()
        .job
        .expect("a job");

    // The lease belongs to gpu-1, the certificate identity -- so a heartbeat
    // sent under the same connection succeeds regardless of what it declares.
    let beat = client
        .heartbeat(v1::HeartbeatRequest {
            worker_id: "anything-at-all".into(),
            job_id: leased.job_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();

    assert!(
        beat.still_leased,
        "the certificate identity holds the lease"
    );
}

/// The pinned certificates are handed to rustls as one PEM bundle, so every
/// worker in it has to be able to complete a handshake -- not just the first.
/// Proven over a real socket because `Certificate::from_pem` only wraps bytes:
/// nothing is parsed until the acceptor is built.
#[tokio::test]
async fn every_worker_in_the_pinned_bundle_can_connect_and_is_told_apart() {
    let f = start_with_tls(&["gpu-1", "gpu-2"]).await;

    let mut first = f.client_as("gpu-1").await;
    let mut second = f.client_as("gpu-2").await;

    let leased = first
        .lease(v1::LeaseRequest {
            worker_id: String::new(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner()
        .job
        .expect("a job");

    // The second handshake succeeded, and its certificate maps to gpu-2: it is
    // told it does not hold gpu-1's lease.
    let beat = second
        .heartbeat(v1::HeartbeatRequest {
            worker_id: "gpu-1".into(),
            job_id: leased.job_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(
        !beat.still_leased,
        "gpu-2's certificate must not inherit gpu-1's lease"
    );

    let beat = first
        .heartbeat(v1::HeartbeatRequest {
            worker_id: String::new(),
            job_id: leased.job_id,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(beat.still_leased, "gpu-1 still holds its own lease");
}

/// The pinning has to reject as well as accept, or the identity it yields means
/// nothing.
///
/// Asserted on the RPC rather than on `connect`: under TLS 1.3 the client
/// finishes its side of the handshake before the server has looked at the
/// client certificate, so `connect` returning `Ok` says nothing about whether
/// the caller was admitted. The first call is where the rejection lands.
#[tokio::test]
async fn a_certificate_that_is_not_pinned_is_refused() {
    let f = start_with_tls(&["gpu-1"]).await;
    // The same subject name as a pinned worker, a different key: the name is
    // not what is pinned.
    let (cert, key) = self_signed("gpu-1");

    // The pinned worker connects first, so nothing below can be explained away
    // by the listener not being up yet.
    let _admitted = f.client_as("gpu-1").await;

    match f.connect_with(cert, key).await {
        // Refused outright, the strictest possible answer.
        Err(_) => {}
        // Admitted only because TLS 1.3 finishes the client's half early;
        // rustls tears the connection down at the first call instead, so this
        // arrives as a transport error and never reaches the service.
        Ok(mut client) => {
            client
                .lease(v1::LeaseRequest {
                    worker_id: "gpu-1".into(),
                    kinds: vec![],
                })
                .await
                .expect_err("an unpinned certificate must not be able to lease");
        }
    }
}

/// Whether the declared id may be trusted is a property of the configuration,
/// not of the connection. Once any worker is pinned, a caller that presents no
/// certificate is refused even on a listener that is not running TLS -- so a
/// transport that carries no certificate cannot quietly reopen the hole.
#[tokio::test]
async fn a_caller_without_a_certificate_is_refused_once_workers_are_pinned() {
    let h = harness().await;
    let pinned = HashMap::from([("a-fingerprint".to_string(), "gpu-1".to_string())]);
    let mut client = serve_plain(h.state.clone().with_worker_names(pinned)).await;

    let err = client
        .lease(v1::LeaseRequest {
            worker_id: "gpu-1".into(),
            kinds: vec![],
        })
        .await
        .expect_err("no certificate, but workers are pinned");

    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");
}

/// Giving a job back is a write like any other: a non-retryable failure also
/// flips the call to Failed, so a worker that does not hold the lease must not
/// be able to do it.
#[tokio::test]
async fn a_worker_that_does_not_hold_the_lease_cannot_fail_the_job() {
    let mut f = start().await;
    let job = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "gpu-box-1".into(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner()
        .job
        .expect("a job");

    let err = f
        .client
        .fail_job(v1::FailJobRequest {
            worker_id: "gpu-box-2".into(),
            job_id: job.job_id,
            error: "not my job to fail".into(),
            retryable: false,
        })
        .await
        .expect_err("gpu-box-2 holds no lease");

    assert_eq!(err.code(), Code::Aborted, "{err:?}");

    // The call must not have been marked failed by a caller with no lease.
    let call = f.call_repo.get_by_id(f.call_id).await.unwrap().unwrap();
    assert_ne!(call.processing_status, ProcessingStatus::Failed);
}

/// Scorecards and search indexing need what was said, not the audio. Without
/// this RPC they cannot run outside the core at all.
#[tokio::test]
async fn a_worker_reads_the_transcript_of_the_call_it_holds() {
    let mut f = start().await;
    // A real `Transcript`, serialized the way `submit_transcript` stores one --
    // every field required, since the domain type carries no defaults.
    let transcript = callmind_transcript::Transcript {
        call_id: f.call_id,
        languages: vec![],
        speakers: vec![],
        segments: vec![callmind_transcript::TranscriptSegment {
            id: uuid::Uuid::new_v4(),
            call_id: f.call_id,
            sequence: 0,
            speaker_id: callmind_core::SpeakerId(1),
            speaker_role: callmind_core::SpeakerRole::Customer,
            language: callmind_core::Language::Hebrew,
            text_direction: callmind_transcript::TextDirection::Rtl,
            start_ms: 0,
            end_ms: 900,
            raw_text: "שלום".into(),
            normalized_text: "שלום".into(),
            words: vec![],
        }],
    };
    f.call_repo
        .save_transcript(f.call_id, &serde_json::to_string(&transcript).unwrap())
        .await
        .unwrap();
    enqueue_job(&f).await;

    let leased = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "w".into(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner()
        .job
        .expect("a job");

    let context = f
        .client
        .get_call_context(v1::GetCallContextRequest {
            worker_id: "w".into(),
            job_id: leased.job_id.clone(),
        })
        .await
        .expect("the holder may read the call")
        .into_inner();

    let proto = context.transcript.expect("a transcript");
    assert_eq!(proto.segments.len(), 1);
    // Absent, not empty: the field is `optional` on the wire precisely so a
    // worker can tell "not analysed yet" from an analysis that says nothing.
    assert!(
        context.analysis_json.is_none(),
        "no analysis is committed yet"
    );

    f.call_repo
        .commit_analysis(
            &AnalysisRow {
                id: uuid::Uuid::new_v4(),
                call_id: f.call_id,
                title: "a returned package",
                summary: "the customer wants a refund",
                reason: None,
                resolution: None,
                resolved: true,
                customer_intent: Some("refund"),
                sentiment_score: 0.5,
                metrics_json: "{}",
                full_analysis_json: r#"{"title":"a returned package"}"#,
                created_at: chrono::Utc::now(),
            },
            ProcessingStatus::Completed,
        )
        .await
        .unwrap();

    let context = f
        .client
        .get_call_context(v1::GetCallContextRequest {
            worker_id: "w".into(),
            job_id: leased.job_id,
        })
        .await
        .expect("the holder may read the call")
        .into_inner();

    assert_eq!(
        context.analysis_json.as_deref(),
        Some(r#"{"title":"a returned package"}"#),
        "the stored analysis reaches the worker verbatim"
    );
}

/// Otherwise this becomes a way to read the whole archive by guessing job ids.
#[tokio::test]
async fn a_worker_without_the_lease_is_refused_the_call() {
    let mut f = start().await;
    enqueue_job(&f).await;

    let leased = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "holder".into(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner()
        .job
        .expect("a job");

    let err = f
        .client
        .get_call_context(v1::GetCallContextRequest {
            worker_id: "someone-else".into(),
            job_id: leased.job_id,
        })
        .await
        .expect_err("no lease, no call");

    assert_eq!(err.code(), Code::Aborted, "{err:?}");
}

/// Nothing else can finish a plugin job: the local pool only leases the kinds it
/// has handlers for, and `plugin:*` is never one of them. Left `Running`, the
/// stale-lock sweep hands the job back to the queue, a worker leases it again
/// and the plugin runs again -- forever, on whatever GPU is serving it.
#[tokio::test]
async fn a_submitted_plugin_result_finishes_the_job() {
    let mut f = start().await;
    f.job_repo
        .enqueue(
            &EnqueueJob::new(
                JobKind::Custom("acoustic-emotions".into()),
                serde_json::json!({ "call_id": f.call_id.to_string() }),
            )
            .with_call_id(f.call_id),
        )
        .await
        .unwrap();

    let lease = v1::LeaseRequest {
        worker_id: "gpu-box-1".into(),
        kinds: vec!["plugin:acoustic-emotions".into()],
    };
    let job = f
        .client
        .lease(lease.clone())
        .await
        .unwrap()
        .into_inner()
        .job
        .expect("the plugin job is leasable");
    assert_eq!(job.kind, "plugin:acoustic-emotions");

    f.client
        .submit_plugin_result(v1::SubmitPluginResultRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
            plugin: "acoustic-emotions".into(),
            payload: Some(v1::submit_plugin_result_request::Payload::Json(
                r#"{"ok":true}"#.into(),
            )),
        })
        .await
        .unwrap();

    let finished = f
        .job_repo
        .get_by_id(job.job_id.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        finished.status,
        JobStatus::Completed,
        "a stored plugin result is the end of the job"
    );

    let again = f.client.lease(lease).await.unwrap().into_inner();
    assert!(
        again.job.is_none(),
        "a finished plugin job must never be leased again"
    );
}

/// That a caller must present a certificate at all is a property of the
/// listener, not of the service: the config-keyed fallback in
/// `a_caller_without_a_certificate_is_refused_once_workers_are_pinned` never
/// reaches a handshake. Without this, "the port demands a certificate" rests
/// entirely on tonic's `client_auth_optional` defaulting to false.
///
/// Asserted on the RPC rather than on `connect`, for the reason spelled out on
/// `a_certificate_that_is_not_pinned_is_refused`: TLS 1.3 lets the client
/// finish its half before the server has judged it.
#[tokio::test]
async fn a_client_with_no_certificate_is_refused_by_the_tls_listener() {
    let f = start_with_tls(&["gpu-1"]).await;
    // A pinned worker connects first, so nothing below is the listener not
    // being up yet.
    let _admitted = f.client_as("gpu-1").await;

    // Trusts the server, offers nothing of its own.
    let tls = tonic::transport::ClientTlsConfig::new()
        .domain_name("callmind.local")
        .ca_certificate(tonic::transport::Certificate::from_pem(f.ca_pem.clone()));
    let endpoint = Channel::from_shared(format!("https://{}", f.addr))
        .unwrap()
        .tls_config(tls)
        .unwrap();

    match endpoint.connect().await {
        Err(_) => {}
        Ok(channel) => {
            let err = WorkerClient::new(channel)
                .lease(v1::LeaseRequest {
                    worker_id: "gpu-1".into(),
                    kinds: vec![],
                })
                .await
                .expect_err("a caller with no certificate must not be able to lease");
            // A transport error, not a `Status` the service produced: rustls
            // sent a `CertificateRequired` alert and the request never reached
            // a handler. Were client authentication optional, this would
            // instead be the service's own `Unauthenticated`.
            assert_eq!(
                err.code(),
                Code::Unknown,
                "the listener must refuse the handshake, not the request: {err:?}"
            );
        }
    }
}

/// A plugin result may only finish a plugin job.
///
/// `submit_plugin_result` completes the job its lease covers, which is right for
/// a `plugin:*` kind -- no local stage follows one. But a remote worker leases
/// the ordinary pipeline kind too, with `kinds: []`, and that is the whole point
/// of a GPU worker. A worker that sends a plugin result against a transcription
/// job would otherwise retire it: no transcript, no analysis, and no error
/// anywhere. A confused third-party worker is enough; malice is not required.
#[tokio::test]
async fn a_plugin_result_cannot_finish_a_transcription_job() {
    let mut f = start().await;

    let leased = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "gpu-1".into(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner()
        .job
        .expect("the pipeline job");
    assert_eq!(
        leased.kind, "ingest_recording",
        "the fixture leases the pipeline kind"
    );

    let err = f
        .client
        .submit_plugin_result(v1::SubmitPluginResultRequest {
            worker_id: "gpu-1".into(),
            job_id: leased.job_id.clone(),
            plugin: "acoustic-emotions".into(),
            payload: Some(v1::submit_plugin_result_request::Payload::Json(
                r#"{"joy":1.0}"#.into(),
            )),
        })
        .await
        .expect_err("a plugin result must not retire a transcription job");

    assert_eq!(err.code(), Code::FailedPrecondition, "{err:?}");

    let job_id: callmind_core::JobId = leased.job_id.parse().unwrap();
    let still_running = f.job_repo.get_by_id(job_id).await.unwrap().unwrap();
    assert_eq!(
        still_running.status,
        JobStatus::Running,
        "the transcription job must still be the worker's to finish"
    );
}
