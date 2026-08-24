//! The surface a plugin depends on.
//!
//! One crate to pin instead of a dozen. Without this a plugin implementing
//! `SttEngine` had to depend on all of `callmind-stt`, and its contract was
//! whatever that crate happened to expose that week.
//!
//! Nothing is *moved* here -- the traits stay where they are implemented and this
//! re-exports a curated subset. Rust has no stable ABI, so a plugin is statically
//! linked and compiles those crates regardless; what a separate crate buys is a
//! named, versioned surface rather than a smaller build.
//!
//! # Writing a plugin
//!
//! ```ignore
//! use callmind_plugin_api::*;
//!
//! struct Emotions;
//!
//! impl Plugin for Emotions {
//!     fn name(&self) -> &str { "acoustic_emotions" }
//!
//!     fn register_jobs(&self, builder: JobRegistryBuilder) -> JobRegistryBuilder {
//!         builder.register(self.job_kind(), EmotionsHandler)
//!     }
//! }
//! ```

/// Everything a plugin is likely to need, in one import.
pub use callmind_analysis::AnalysisEngine;
pub use callmind_audio::AudioBuffer;
pub use callmind_core::{
    Call, CallDirection, CallId, EnqueueJob, Job, JobId, JobKind, JobStatus, Language, OrgId,
    ProcessingStatus, Recording, SpeakerId, SpeakerRole,
};
pub use callmind_db::{CallRepository, JobRepository, SpeakerRepository};
pub use callmind_diarization::traits::DiarizationEngine;
pub use callmind_jobs::{
    CallAnalysisContext, JobContext, JobExecutionError, JobHandler, JobRegistry,
    JobRegistryBuilder, Plugin, run_transcript_plugins,
};
pub use callmind_language::traits::LanguageEngine;
pub use callmind_llm::LlmEngine;
pub use callmind_storage::RecordingStorage;
pub use callmind_stt::traits::SttEngine;
pub use callmind_transcript::Transcript;
pub use callmind_ui::templates::TemplateRegistry;
pub use callmind_vad::{SpeechRegion, VadEngine};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Silent;

    impl Plugin for Silent {
        fn name(&self) -> &'static str {
            "silent"
        }
    }

    struct Failing;

    #[async_trait::async_trait]
    impl Plugin for Failing {
        fn name(&self) -> &'static str {
            "failing"
        }

        async fn on_transcript(
            &self,
            _call: &CallAnalysisContext<'_>,
        ) -> Result<Option<serde_json::Value>, String> {
            Err("the model was not loaded".to_string())
        }
    }

    fn a_context() -> (AudioBuffer, Transcript) {
        (
            AudioBuffer::new(16_000, 1, vec![0.0; 16_000]),
            Transcript {
                call_id: CallId::generate(),
                languages: Vec::new(),
                speakers: Vec::new(),
                segments: Vec::new(),
            },
        )
    }

    /// A plugin computes something from the audio and transcript, and the host
    /// stores it under the plugin's name -- which is the same name its view is
    /// registered under, so the result renders without further wiring.
    #[tokio::test]
    async fn a_plugin_result_comes_back_tagged_with_the_plugin_name() {
        let (audio, transcript) = a_context();
        let plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(EmotionsPlugin), Arc::new(Silent)];

        let results = run_transcript_plugins(
            &plugins,
            &CallAnalysisContext {
                call_id: transcript.call_id,
                audio: &audio,
                transcript: &transcript,
            },
        )
        .await;

        assert_eq!(results.len(), 1, "the silent plugin contributes nothing");
        assert_eq!(results[0].0, "acoustic_emotions");
        assert_eq!(results[0].1["speakers"], serde_json::json!(0));
    }

    /// One plugin failing must not cost the others their results, nor the call
    /// its transcript.
    #[tokio::test]
    async fn a_failing_plugin_does_not_take_the_others_down() {
        let (audio, transcript) = a_context();
        let plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(Failing), Arc::new(EmotionsPlugin)];

        let results = run_transcript_plugins(
            &plugins,
            &CallAnalysisContext {
                call_id: transcript.call_id,
                audio: &audio,
                transcript: &transcript,
            },
        )
        .await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "acoustic_emotions");
    }

    struct EmotionsPlugin;

    #[async_trait::async_trait]
    impl callmind_jobs::JobHandler for EmotionsPlugin {
        async fn execute(
            &self,
            _ctx: callmind_jobs::JobContext,
        ) -> Result<(), callmind_jobs::JobExecutionError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl Plugin for EmotionsPlugin {
        fn name(&self) -> &'static str {
            "acoustic_emotions"
        }

        async fn on_transcript(
            &self,
            call: &CallAnalysisContext<'_>,
        ) -> Result<Option<serde_json::Value>, String> {
            Ok(Some(serde_json::json!({
                "speakers": call.transcript.speakers.len()
            })))
        }

        fn register_jobs(&self, builder: JobRegistryBuilder) -> JobRegistryBuilder {
            builder.register(self.job_kind(), EmotionsPlugin)
        }

        fn register_ui(&self, templates: &mut TemplateRegistry) -> Result<(), String> {
            templates
                .register_plugin_template("acoustic_emotions", "<div>{{ value }}</div>".to_string())
                .map_err(|e| e.to_string())
        }
    }

    /// A plugin wires itself in: the host loops over plugins and does not name
    /// any of them. This is what lets a closed-source plugin ship separately.
    #[test]
    fn a_plugin_registers_its_own_job_kind_and_ui() {
        let plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(EmotionsPlugin)];
        let mut templates = TemplateRegistry::new();

        let mut builder = callmind_jobs::JobRegistry::builder();
        for plugin in &plugins {
            builder = plugin.register_jobs(builder);
            plugin.register_ui(&mut templates).expect("ui registers");
        }
        let registry = builder.build();

        assert!(
            registry
                .get(&callmind_core::JobKind::Custom("acoustic_emotions".into()))
                .is_some(),
            "the plugin's stage is reachable under its own name"
        );
        let rendered = templates
            .render_plugin("acoustic_emotions", &serde_json::json!({"value": "ok"}))
            .expect("a view is registered for it")
            .expect("and it renders");
        assert!(
            rendered.contains("ok"),
            "the payload reaches the view: {rendered}"
        );
    }

    /// The name is the plugin's identity in two places -- the job kind and the
    /// template -- so the helper must derive one from the other rather than
    /// letting them drift.
    #[test]
    fn the_job_kind_is_derived_from_the_plugin_name() {
        assert_eq!(
            EmotionsPlugin.job_kind(),
            callmind_core::JobKind::Custom("acoustic_emotions".to_string())
        );
        assert_eq!(
            EmotionsPlugin.job_kind().as_str(),
            "plugin:acoustic_emotions"
        );
    }
}
