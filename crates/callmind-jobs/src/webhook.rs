//! Outbound delivery of a finished call.
//!
//! This is what makes CallMind a producer rather than only a consumer: a
//! finished call can land in n8n, a Shortcut, or any endpoint that speaks HTTP.
//!
//! Delivery is a queued job rather than a call at the end of the pipeline, so a
//! receiver that is down costs a retry with the queue's backoff instead of
//! re-running transcription and analysis. The job payload *is* the request body,
//! which means every retry delivers byte-identical content and a receiver can
//! deduplicate on the call id.

use std::time::Duration;

use async_trait::async_trait;
use tracing::{info, warn};

use crate::errors::JobExecutionError;
use crate::handler::{JobContext, JobHandler};

/// How much of a failing receiver's response body to keep in the job error.
const MAX_LOGGED_BODY: usize = 200;

/// Header carrying the shared secret, so a receiver can tell the request is ours.
pub const SECRET_HEADER: &str = "X-CallMind-Secret";

pub struct WebhookDeliveryHandler {
    url: String,
    secret: Option<String>,
    timeout: Duration,
    client: reqwest::Client,
}

impl WebhookDeliveryHandler {
    /// The timeout is applied per request rather than to a built client, which
    /// keeps construction infallible -- a client builder that fails would
    /// otherwise have to be unwrapped, and the usual fallback silently drops the
    /// timeout.
    #[must_use]
    pub fn new(url: String, secret: Option<String>, timeout: Duration) -> Self {
        Self {
            url,
            secret,
            timeout,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl JobHandler for WebhookDeliveryHandler {
    async fn execute(&self, ctx: JobContext) -> Result<(), JobExecutionError> {
        let mut request = self
            .client
            .post(&self.url)
            .timeout(self.timeout)
            .json(&ctx.job.payload);

        if let Some(secret) = &self.secret {
            request = request.header(SECRET_HEADER, secret);
        }

        // `without_url` because a receiver URL may carry a token in its query
        // string, and this error text is stored on the job and logged.
        let response = request.send().await.map_err(|e| {
            JobExecutionError::Retryable(format!("Webhook delivery failed: {}", e.without_url()))
        })?;

        let status = response.status();
        if status.is_success() {
            info!(status = status.as_u16(), "Webhook delivered");
            return Ok(());
        }

        let mut body = response.text().await.unwrap_or_default();
        body.truncate(MAX_LOGGED_BODY);
        let detail = format!("Webhook receiver returned {status}: {body}");

        if status.is_client_error() {
            // Replaying an identical request cannot turn a 4xx into a 2xx: the
            // URL, the secret or the body is what the receiver objects to.
            warn!(status = status.as_u16(), "Webhook rejected, not retrying");
            Err(JobExecutionError::Failed(detail))
        } else {
            Err(JobExecutionError::Retryable(detail))
        }
    }
}
