use callmind_audio::{AudioDecoder, AudioResampler};
use std::f32::consts::PI;

/// 2 seconds of a 440 Hz sine, encoded as OGG/Opus. Synthetic on purpose: no
/// real recording is committed to the repository.
const FIXTURE: &[u8] = include_bytes!("fixtures/sine_440hz_48k_mono.opus.ogg");

fn amplitude_at(samples: &[f32], freq: f32, sample_rate: u32) -> f32 {
    let (mut re, mut im) = (0.0f32, 0.0f32);
    for (i, &s) in samples.iter().enumerate() {
        let angle = 2.0 * PI * freq * (i as f32) / (sample_rate as f32);
        re += s * angle.cos();
        im -= s * angle.sin();
    }
    2.0 * (re * re + im * im).sqrt() / (samples.len() as f32)
}

/// WhatsApp and Telegram voice notes are OGG/Opus. Symphonia demuxes the OGG
/// container but has no Opus decoder — there is no `symphonia-codec-opus` — so
/// ingestion used to fail outright with "unsupported codec".
#[test]
fn decodes_ogg_opus_voice_note() {
    let decoded = AudioDecoder::decode_bytes(FIXTURE, Some("ogg"))
        .expect("OGG/Opus must decode; this is the format voice notes arrive in");

    assert_eq!(decoded.sample_rate, 48_000);
    assert_eq!(decoded.channels, 1);

    let duration = decoded.duration_ms();
    assert!(
        (2000i64 - duration as i64).abs() < 120,
        "expected ~2000ms of audio, got {duration}ms"
    );

    // Prove it really decoded rather than returning silence or noise. Levels
    // are cross-checked against an ffmpeg reference decode of the same fixture:
    // amp@440 = 0.2496, amp@1000 = 0.0.
    let tone = amplitude_at(&decoded.samples, 440.0, 48_000);
    let off_tone = amplitude_at(&decoded.samples, 1_000.0, 48_000);
    assert!(tone > 0.15, "440 Hz tone missing (amp {tone:.4})");
    assert!(
        tone > off_tone * 50.0,
        "440 Hz should dominate: tone {tone:.4} vs off-tone {off_tone:.4}"
    );
}

/// Opus is detected from the container magic, so a wrong or missing extension
/// hint must not change the outcome.
#[test]
fn opus_detection_ignores_the_extension_hint() {
    for hint in [None, Some("m4a"), Some("oga"), Some("opus")] {
        let decoded = AudioDecoder::decode_bytes(FIXTURE, hint)
            .unwrap_or_else(|e| panic!("hint {hint:?} broke Opus detection: {e}"));
        assert_eq!(decoded.sample_rate, 48_000);
    }
}

/// The pipeline feeds Whisper 16 kHz mono, so the Opus path has to survive it.
#[test]
fn opus_survives_resampling_to_16k() {
    let decoded = AudioDecoder::decode_bytes(FIXTURE, Some("ogg")).unwrap();
    let resampled = AudioResampler::resample_to_16k_mono(&decoded).unwrap();

    assert_eq!(resampled.sample_rate, 16_000);
    let amp = amplitude_at(&resampled.samples, 440.0, 16_000);
    assert!(amp > 0.15, "tone lost during resampling (amp {amp:.4})");
}

/// Batch import reads duration/channels/rate from the container headers instead
/// of decoding every packet. Measured on a real 42.9-minute recording: 504 MB
/// peak RSS and 1.27s for a full decode, against 7.1 MB and 183ms for the
/// header read — so the values had better agree.
#[test]
fn metadata_matches_a_full_decode() {
    let decoded = AudioDecoder::decode_bytes(FIXTURE, Some("ogg")).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.ogg");
    std::fs::write(&path, FIXTURE).unwrap();

    // A None result is also valid: some streamed containers omit the frame
    // count, and that is the contract making the importer fall back to a full
    // decode. Only a present value has to agree.
    if let Some(meta) = AudioDecoder::read_metadata(&path).unwrap() {
        assert_eq!(meta.sample_rate, decoded.sample_rate);
        assert_eq!(meta.channels, decoded.channels);
        let drift = (meta.duration_ms as i64 - decoded.duration_ms() as i64).abs();
        assert!(
            drift < 100,
            "header says {}ms, decode says {}ms",
            meta.duration_ms,
            decoded.duration_ms()
        );
    }
}

/// A non-audio file must be reported as an error, not silently accepted.
#[test]
fn metadata_rejects_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-audio.m4a");
    std::fs::write(&path, b"this is definitely not audio").unwrap();
    assert!(AudioDecoder::read_metadata(&path).is_err());
}
