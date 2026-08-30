//! One way to assemble the transcription stack, shared by every process that
//! runs it.
//!
//! The server builds this at startup; a remote worker builds the same thing on
//! another machine and returns transcripts over gRPC. Assembling it twice would
//! mean two copies of a dozen decisions -- the router's confidence threshold,
//! the segmenter fallback, how language identification is wired to the
//! multilingual model -- and a remote transcript that quietly differs from a
//! local one is the worst kind of drift, because both look fine.

use crate::AudioTranscriber;
use callmind_config::ModelsConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

/// The transcription stack, plus the handles a caller needs after building it.
pub struct BuiltTranscriber {
    pub transcriber: Arc<AudioTranscriber>,
    /// The speech-to-text engines, kept so shutdown can release GPU weights.
    /// They are already inside `transcriber`; this is the same `Arc`s, not a
    /// second set.
    pub stt_engines: Vec<Arc<callmind_stt::WhisperCppEngine>>,
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// A missing weights file otherwise surfaces as a failed job on the first
    /// call, long after startup, so it is refused here with the command that
    /// fixes it.
    #[error(
        "{kind} model not found at {path}. Fetch it with `callmind models download <id>` \
         (`callmind models list` shows the ids), or point `models.stt_*` in the config \
         at a file you already have."
    )]
    MissingModel { kind: &'static str, path: PathBuf },
}

/// Build the transcription stack from configured model paths.
///
/// Everything optional degrades rather than fails: without the segmentation
/// model, speech detection falls back to the energy detector and the speaker
/// count to the two-party prior. Only the two speech-to-text models are
/// required, because there is no transcription without them.
pub fn build_transcriber(models: &ModelsConfig) -> Result<BuiltTranscriber, BuildError> {
    // Speaker segmentation, when its model is present, is both a better speech
    // detector and the source of the speaker count. Loaded once and shared: the
    // diarizer uses it for both, and language identification -- which probes a
    // few windows of speech to choose a Whisper model -- gets regions that are
    // actually speech rather than possibly hold music. Speech-to-text is given
    // the whole recording either way.
    let segmentation_model = models.models_dir.join("diarization/segmentation.onnx");
    let segmenter = if segmentation_model.exists() {
        match callmind_diarization::pyannote::PyannoteSegmenter::load(&segmentation_model) {
            Ok(loaded) => {
                info!(
                    "Loaded pyannote speaker segmentation from {}; speech detection and the \
                     speaker count are measured rather than assumed",
                    segmentation_model.display()
                );
                Some(Arc::new(loaded))
            }
            Err(e) => {
                warn!(
                    "Failed to load speaker segmentation from {}: {e}. Falling back to the \
                     energy detector and the two-party prior.",
                    segmentation_model.display()
                );
                None
            }
        }
    } else {
        None
    };

    let vad: Arc<dyn callmind_vad::VadEngine> = match segmenter.clone() {
        Some(seg) => Arc::new(callmind_diarization::pyannote::PyannoteVad::new(seg)),
        None => Arc::new(callmind_vad::EnergyVadEngine::default()),
    };

    // From config, not compiled in: speech-to-text dominates processing time and
    // the model is the lever on it.
    let hebrew_model_path = models.models_dir.join(&models.stt_hebrew);
    let multi_model_path = models.models_dir.join(&models.stt_multilingual);

    ensure_model_present("hebrew speech-to-text", &hebrew_model_path)?;
    ensure_model_present("multilingual speech-to-text", &multi_model_path)?;

    let hebrew_label = model_label(&hebrew_model_path);
    let multi_label = model_label(&multi_model_path);
    let hebrew_stt = Arc::new(callmind_stt::WhisperCppEngine::new(
        hebrew_model_path,
        &hebrew_label,
        "1.0",
    ));
    let multi_stt = Arc::new(callmind_stt::WhisperCppEngine::new(
        multi_model_path,
        &multi_label,
        "1.0",
    ));
    let stt_engines = vec![hebrew_stt.clone(), multi_stt.clone()];

    let stt_router = Arc::new(callmind_stt::SttRouter::new(
        hebrew_stt,
        multi_stt.clone(),
        0.90,
    ));

    // Production acoustic LID backed by the Whisper multilingual model's probe.
    let multi_stt_for_lid = multi_stt.clone();
    let language_engine = Arc::new(
        callmind_language::SamplingLanguageEngine::new().with_detector(move |buf| {
            match multi_stt_for_lid.detect_language_probe(buf) {
                Ok(probs) => probs
                    .into_iter()
                    .map(
                        |(language, probability)| callmind_language::LanguageProbability {
                            language,
                            probability,
                        },
                    )
                    .collect(),
                Err(e) => {
                    // Swallowing this made a missing Whisper model look like
                    // "no languages detected", and the analyzer then defaults
                    // the language to Hebrew -- silently mistranscribing.
                    warn!("Acoustic language probe failed: {e}");
                    Vec::new()
                }
            }
        }),
    );

    let stereo_diarizer = Arc::new(callmind_diarization::StereoChannelDiarizer::new(
        vad.clone(),
    ));
    let onnx_diarization_model = models.models_dir.join("diarization/speaker_embedding.onnx");
    let mut neural_diarizer = callmind_diarization::NeuralDiarizer::new_with_fallback(
        Some(onnx_diarization_model),
        vad.clone(),
    );

    // Measured on labelled recordings: with segmentation the speaker count comes
    // out 4/4 on monologues and 23/24 on two-party calls, against 0/4 and 24/24
    // for the two-party assumption it replaces.
    if let Some(seg) = segmenter {
        neural_diarizer = neural_diarizer.with_segmenter(seg);
    }
    let clustering_diarizer = Arc::new(neural_diarizer);

    // One permit: the GPU is the resource being protected, and this process owns
    // exactly one of them whether it is the server or a worker.
    let gpu_semaphore = Arc::new(tokio::sync::Semaphore::new(1));

    Ok(BuiltTranscriber {
        transcriber: Arc::new(AudioTranscriber::new(
            vad,
            language_engine,
            stt_router,
            stereo_diarizer,
            clustering_diarizer,
            gpu_semaphore,
        )),
        stt_engines,
    })
}

fn ensure_model_present(kind: &'static str, path: &Path) -> Result<(), BuildError> {
    if path.exists() {
        Ok(())
    } else {
        Err(BuildError::MissingModel {
            kind,
            path: path.to_path_buf(),
        })
    }
}

/// The label is stored with every transcript, so it follows the configured file
/// instead of guessing which model is loaded.
fn model_label(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Weights files exist but hold nothing: `WhisperCppEngine::new` records a
    /// path and loads on first use, so building the stack never reads them.
    /// That is what makes this test cheap enough to run everywhere.
    fn models_dir_with_stubs() -> (tempfile::TempDir, ModelsConfig) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("stt")).expect("stt dir");
        for name in ["stt/hebrew.bin", "stt/multi.bin"] {
            std::fs::write(dir.path().join(name), b"not really weights").expect("write");
        }
        let config = ModelsConfig {
            models_dir: dir.path().to_path_buf(),
            stt_hebrew: "stt/hebrew.bin".to_string(),
            stt_multilingual: "stt/multi.bin".to_string(),
        };
        (dir, config)
    }

    /// A missing weights file otherwise surfaces as a failed job on the first
    /// call, long after startup -- and switching the default multilingual model
    /// to turbo makes that likely for anyone who downloaded the old default.
    #[test]
    fn a_missing_model_is_refused_with_the_command_that_fixes_it() {
        let (_dir, mut config) = models_dir_with_stubs();
        config.stt_multilingual = "stt/turbo.bin".to_string();

        // `let ... else` rather than `expect_err`: the success type holds trait
        // objects and so cannot be `Debug`.
        let Err(err) = build_transcriber(&config) else {
            panic!("a missing model must not be accepted");
        };
        let msg = err.to_string();
        assert!(msg.contains("stt/turbo.bin"), "names the file: {msg}");
        assert!(msg.contains("models download"), "names the fix: {msg}");
    }

    /// Nothing but speech-to-text is required. A deployment without the
    /// diarization models still transcribes, on the energy detector and the
    /// two-party prior.
    #[test]
    fn the_stack_builds_without_any_diarization_models() {
        let (_dir, config) = models_dir_with_stubs();
        let built = build_transcriber(&config).expect("speech-to-text is all that is required");
        assert_eq!(
            built.stt_engines.len(),
            2,
            "both engines are returned so shutdown can release their weights"
        );
    }

    /// The label is stored with every transcript, so it has to follow the
    /// configured file rather than a compiled-in guess at which model is loaded.
    #[test]
    fn the_engine_label_follows_the_configured_filename() {
        assert_eq!(
            model_label(Path::new("models/stt/whisper-large-v3-turbo.bin")),
            "whisper-large-v3-turbo"
        );
        assert_eq!(
            model_label(Path::new("models/stt/whisper-large-v3.bin")),
            "whisper-large-v3"
        );
    }
}
