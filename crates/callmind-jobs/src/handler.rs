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
        self.handlers.keys().copied().collect()
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
