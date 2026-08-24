//! The extension point for the call pipeline.
//!
//! Defined here rather than in `callmind-plugin-api` because the pipeline has to
//! call it: a trait the pipeline depends on cannot live in a crate that depends
//! on the pipeline. `callmind-plugin-api` re-exports all of this, so a plugin
//! author still imports one crate.

use callmind_audio::AudioBuffer;
use callmind_core::{CallId, JobKind};
use callmind_transcript::Transcript;
use callmind_ui::templates::TemplateRegistry;

use crate::handler::JobRegistryBuilder;
use std::sync::Arc;

/// What a plugin is given when a call has been transcribed.
///
/// Borrowed, not owned: the audio is hundreds of megabytes decoded and the host
/// already has it in hand. A plugin that needs to keep something copies just that.
pub struct CallAnalysisContext<'a> {
    pub call_id: CallId,
    /// The decoded recording, 16 kHz mono.
    pub audio: &'a AudioBuffer,
    /// Speaker turns and text, already aligned.
    pub transcript: &'a Transcript,
}

/// Run every plugin's transcript hook and collect what they produced.
///
/// One plugin failing costs only its own result: the transcript is already
/// committed by the time this runs, and a plugin is not allowed to take a call
/// down with it.
pub async fn run_transcript_plugins(
    plugins: &[Arc<dyn Plugin>],
    call: &CallAnalysisContext<'_>,
) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    for plugin in plugins {
        match plugin.on_transcript(call).await {
            Ok(Some(value)) => out.push((plugin.name().to_string(), value)),
            Ok(None) => {}
            Err(e) => {
                // The plugin's problem, reported and stepped over.
                tracing::warn!(
                    "Plugin '{}' failed on call {}: {e}",
                    plugin.name(),
                    call.call_id
                );
            }
        }
    }
    out
}

/// What a plugin implements so the host can wire it in without naming it.
///
/// Every method but [`Plugin::name`] has a default, so a plugin that only adds a
/// view implements one thing and a plugin that only adds a stage implements
/// another. The host loops over a list of these; adding a plugin is one line in
/// that list rather than five scattered through startup.
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    /// Identifies the plugin. Used for its job kind and its template name, so it
    /// must be stable -- renaming it orphans anything already queued.
    fn name(&self) -> &str;

    /// The job kind this plugin's stage runs under.
    ///
    /// Derived from [`Plugin::name`] rather than declared separately, so the two
    /// cannot drift apart.
    fn job_kind(&self) -> JobKind {
        JobKind::Custom(self.name().to_string())
    }

    /// Add job handlers. Returns the builder so several plugins can chain.
    fn register_jobs(&self, builder: JobRegistryBuilder) -> JobRegistryBuilder {
        builder
    }

    /// Add views to the UI.
    ///
    /// Takes the registry by mutable reference because registering a template
    /// mutates it; the host registers everything at startup, before the registry
    /// is shared with request handlers. Errors are reported as strings so the
    /// trait does not pull a specific error type into a plugin's dependency
    /// surface.
    fn register_ui(&self, _templates: &mut TemplateRegistry) -> Result<(), String> {
        Ok(())
    }

    /// Compute something once a call has been transcribed.
    ///
    /// This is the one place a plugin sees the audio and the transcript together,
    /// which is what an acoustic analysis needs. Whatever is returned is stored
    /// against the call under [`Plugin::name`] -- the same name its view is
    /// registered under, so the result renders with no further wiring.
    ///
    /// A single hook rather than a stage for every step of the pipeline: the
    /// alternative was chaining a job per stage, and the stages that need audio
    /// would each have re-decoded it. Measured on this archive that is 1.45 s and
    /// a 504 MB peak per decode, four times over.
    async fn on_transcript(
        &self,
        _call: &CallAnalysisContext<'_>,
    ) -> Result<Option<serde_json::Value>, String> {
        Ok(None)
    }
}
