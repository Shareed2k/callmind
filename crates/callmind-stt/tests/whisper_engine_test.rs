//! The real Whisper engine, exercised through `WhisperCppEngine`.
//!
//! Everything else that touches speech-to-text uses `MockSttEngine`, so until
//! this existed nothing in the suite ever ran whisper.cpp. That gap let a change
//! ship that made every transcription fail with `whisper_full` returning -6:
//! whisper-rs 0.16's `set_abort_callback_safe` hands the C side a trampoline
//! typed over the caller's concrete closure while storing a `Box<dyn FnMut>`, so
//! the callback read a fat pointer as if it were the closure and returned a
//! garbage bool. A non-zero byte means "abort", and encoding stopped.
//!
//! It reached production because the change was verified the wrong way -- the
//! server was killed mid-transcription and shut down cleanly, which proved the
//! new abort path worked and proved nothing about the ordinary one. These tests
//! are the ordinary one.
//!
//! Both engine entry points are covered, because both build `FullParams` and
//! both carried the defect: `transcribe` and `detect_language_probe`.
//!
//! Set `CALLMIND_TEST_STT_MODEL` to any ggml Whisper model. CI points it at
//! `whisper-tiny`, which is 74 MB and multilingual -- large enough to exercise the
//! decoder, small enough to download on every run. Without it these skip, the
//! way the Postgres tests skip without a database.

use callmind_audio::AudioBuffer;
use callmind_stt::{SttEngine, SttRequest, WhisperCppEngine};

/// The model to run against, or `None` when the suite should skip.
fn model_path() -> Option<std::path::PathBuf> {
    if let Some(configured) = std::env::var_os("CALLMIND_TEST_STT_MODEL") {
        let path = std::path::PathBuf::from(configured);
        assert!(
            path.exists(),
            "CALLMIND_TEST_STT_MODEL points at {}, which does not exist. \
             An explicitly configured model that is missing is a broken run, not a skip.",
            path.display()
        );
        return Some(path);
    }

    // The workspace layout puts models beside the binary; tests run from the
    // crate directory, so look both ways rather than guessing one.
    for candidate in [
        "models/stt/whisper-tiny.bin",
        "../../models/stt/whisper-tiny.bin",
    ] {
        let path = std::path::PathBuf::from(candidate);
        if path.exists() {
            return Some(path);
        }
    }

    eprintln!(
        "skipping: no Whisper model. Set CALLMIND_TEST_STT_MODEL, or run \
         `callmind models download whisper-tiny`."
    );
    None
}

/// Three seconds of 16 kHz mono audio, which is what the pipeline hands the
/// engine after resampling.
///
/// A tone rather than speech: what these tests check is that the decoder runs at
/// all, and a failure to encode fails on any input. Asserting on transcribed
/// words would need a speech fixture and would make the test about the model's
/// accuracy rather than about our own code.
fn three_seconds_of_audio() -> AudioBuffer {
    const SAMPLE_RATE: u32 = 16_000;
    let samples: Vec<f32> = (0..SAMPLE_RATE * 3)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            0.2 * (2.0 * std::f32::consts::PI * 220.0 * t).sin()
        })
        .collect();
    AudioBuffer::new(SAMPLE_RATE, 1, samples)
}

#[tokio::test]
async fn the_engine_transcribes_without_the_decoder_giving_up() {
    let Some(model) = model_path() else {
        return;
    };
    let engine = WhisperCppEngine::new(model, "test-tiny", "1.0");
    let audio = three_seconds_of_audio();

    let result = engine
        .transcribe(SttRequest {
            audio: &audio,
            language_hint: None,
            vocabulary: &[],
            word_timestamps: true,
        })
        .await;

    // The failure this guards against surfaces here as
    // `Speech-to-text inference failed: ... Error code: -6`.
    let transcript = result.expect("the engine must transcribe rather than fail to encode");

    // Nothing is asserted about the words: a tone has none to find, and the
    // decoder failing is what this guards against. What must hold is that the
    // engine came back with a result whose parts agree -- every word inside the
    // audio it was given.
    for word in &transcript.words {
        assert!(
            word.end_ms <= 3_000,
            "a word cannot end after the audio does: {word:?}"
        );
    }
}

#[tokio::test]
async fn the_engine_probes_the_language_without_the_decoder_giving_up() {
    let Some(model) = model_path() else {
        return;
    };
    let engine = WhisperCppEngine::new(model, "test-tiny", "1.0");
    let audio = three_seconds_of_audio();

    let detected = engine
        .detect_language_probe(&audio)
        .expect("the probe must run rather than fail to encode");

    assert!(
        !detected.is_empty(),
        "the probe should name at least one candidate language"
    );
    let (_, confidence) = detected[0];
    assert!(
        (0.0..=1.0).contains(&confidence),
        "confidence should be a probability, got {confidence}"
    );
}

/// A model path that is not a model must fail, not hang or succeed emptily.
#[tokio::test]
async fn a_missing_model_is_reported_rather_than_transcribed_around() {
    let engine = WhisperCppEngine::new("models/stt/definitely-not-here.bin", "absent", "1.0");
    let audio = three_seconds_of_audio();

    let err = engine
        .transcribe(SttRequest {
            audio: &audio,
            language_hint: None,
            vocabulary: &[],
            word_timestamps: true,
        })
        .await
        .expect_err("a missing model cannot produce a transcript");

    assert!(
        format!("{err}").contains("definitely-not-here.bin"),
        "the error should name the file: {err}"
    );
}
