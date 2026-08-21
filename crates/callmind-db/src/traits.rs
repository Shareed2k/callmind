use crate::errors::DbError;
use async_trait::async_trait;
use callmind_core::{
    Call, CallFilter, CallId, EnqueueJob, Job, JobId, JobKind, OrgId, ProcessingStatus, Recording,
};
use std::time::Duration;

#[async_trait]
pub trait CallRepository: Send + Sync {
    async fn create(&self, call: &Call) -> Result<(), DbError>;
    async fn get_by_id(&self, id: CallId) -> Result<Option<Call>, DbError>;
    async fn get_by_external_id(
        &self,
        org_id: OrgId,
        ext_id: &str,
    ) -> Result<Option<Call>, DbError>;
    async fn list(&self, filter: &CallFilter) -> Result<Vec<Call>, DbError>;
    async fn update_status(&self, id: CallId, status: ProcessingStatus) -> Result<(), DbError>;
    async fn delete(&self, id: CallId) -> Result<bool, DbError>;
    async fn toggle_favorite(&self, id: CallId) -> Result<bool, DbError>;
    async fn update_tags(&self, id: CallId, tags: &[String]) -> Result<(), DbError>;

    async fn add_recording(&self, recording: &Recording) -> Result<(), DbError>;
    async fn get_recording_by_call_id(&self, call_id: CallId)
    -> Result<Option<Recording>, DbError>;
}

#[async_trait]
pub trait JobRepository: Send + Sync {
    async fn enqueue(&self, job: &EnqueueJob) -> Result<JobId, DbError>;
    async fn fetch_and_lock(
        &self,
        worker_id: &str,
        kinds: &[JobKind],
    ) -> Result<Option<Job>, DbError>;
    async fn renew_lock(&self, id: JobId, worker_id: &str) -> Result<bool, DbError>;
    async fn mark_completed(&self, id: JobId) -> Result<(), DbError>;
    async fn mark_failed(
        &self,
        id: JobId,
        error: &str,
        retry_delay: Option<Duration>,
    ) -> Result<(), DbError>;
    async fn release_stale_locks(&self, older_than: Duration) -> Result<u64, DbError>;
    async fn get_by_id(&self, id: JobId) -> Result<Option<Job>, DbError>;
    async fn list_by_call_id(&self, call_id: CallId) -> Result<Vec<Job>, DbError>;
}
