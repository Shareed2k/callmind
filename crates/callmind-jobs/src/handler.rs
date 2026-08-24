use crate::errors::JobExecutionError;
use async_trait::async_trait;
use callmind_core::{Job, JobKind};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Context passed to a job handler during execution.
#[derive(Clone)]
pub struct JobContext {
    pub job: Job,
    pub cancellation_token: CancellationToken,
}

/// Handler trait for processing a specific JobKind.
#[async_trait]
pub trait JobHandler: Send + Sync {
    async fn execute(&self, ctx: JobContext) -> Result<(), JobExecutionError>;
}

/// Thread-safe registry holding job handlers for different JobKinds.
#[derive(Default, Clone)]
pub struct JobRegistry {
    handlers: Arc<HashMap<JobKind, Arc<dyn JobHandler>>>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn builder() -> JobRegistryBuilder {
        JobRegistryBuilder::default()
    }

    pub fn get(&self, kind: &JobKind) -> Option<Arc<dyn JobHandler>> {
        self.handlers.get(kind).cloned()
    }

    pub fn registered_kinds(&self) -> Vec<JobKind> {
        self.handlers.keys().cloned().collect()
    }
}

#[derive(Default)]
pub struct JobRegistryBuilder {
    handlers: HashMap<JobKind, Arc<dyn JobHandler>>,
}

impl JobRegistryBuilder {
    #[must_use]
    pub fn register<H: JobHandler + 'static>(mut self, kind: JobKind, handler: H) -> Self {
        self.handlers.insert(kind, Arc::new(handler));
        self
    }

    #[must_use]
    pub fn register_arc(mut self, kind: JobKind, handler: Arc<dyn JobHandler>) -> Self {
        self.handlers.insert(kind, handler);
        self
    }

    #[must_use]
    pub fn build(self) -> JobRegistry {
        JobRegistry {
            handlers: Arc::new(self.handlers),
        }
    }
}

#[cfg(test)]
mod plugin_registration_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting(Arc<AtomicUsize>);

    fn a_job(kind: JobKind) -> callmind_core::Job {
        callmind_core::Job {
            id: callmind_core::JobId::generate(),
            call_id: None,
            kind,
            payload: serde_json::json!({}),
            status: callmind_core::JobStatus::Running,
            priority: 0,
            attempt: 1,
            max_attempts: 3,
            run_after: chrono::Utc::now(),
            locked_at: None,
            locked_by: None,
            last_error: None,
            created_at: chrono::Utc::now(),
            completed_at: None,
        }
    }

    #[async_trait]
    impl JobHandler for Counting {
        async fn execute(&self, _ctx: JobContext) -> Result<(), JobExecutionError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// The whole point of `JobKind::Custom`: a plugin registers a stage under its
    /// own name and the worker can find it, with nothing in this crate naming it.
    #[tokio::test]
    async fn a_plugin_stage_can_be_registered_and_found() {
        let runs = Arc::new(AtomicUsize::new(0));
        let registry = JobRegistry::builder()
            .register(
                JobKind::Custom("acoustic_emotions".to_string()),
                Counting(runs.clone()),
            )
            .build();

        let handler = registry
            .get(&JobKind::Custom("acoustic_emotions".to_string()))
            .expect("the plugin's own kind resolves to its handler");

        handler
            .execute(JobContext {
                job: a_job(JobKind::Custom("acoustic_emotions".to_string())),
                cancellation_token: CancellationToken::new(),
            })
            .await
            .expect("it runs");
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        assert!(
            registry
                .get(&JobKind::Custom("something_else".to_string()))
                .is_none(),
            "a different plugin name must not resolve to this handler"
        );
        assert!(
            registry.get(&JobKind::IngestRecording).is_none(),
            "and a plugin must not shadow a built-in kind"
        );
    }

    #[test]
    fn registered_kinds_reports_plugin_stages() {
        let registry = JobRegistry::builder()
            .register(
                JobKind::DeliverWebhook,
                Counting(Arc::new(AtomicUsize::new(0))),
            )
            .register(
                JobKind::Custom("emotions".to_string()),
                Counting(Arc::new(AtomicUsize::new(0))),
            )
            .build();

        let mut kinds: Vec<String> = registry
            .registered_kinds()
            .iter()
            .map(|k| k.as_str().to_string())
            .collect();
        kinds.sort();
        assert_eq!(kinds, vec!["deliver_webhook", "plugin:emotions"]);
    }
}
