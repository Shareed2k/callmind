use callmind_audio::{AudioDecoder, AudioResampler, ChannelAnalyzer};
use callmind_vad::{EnergyVadEngine, VadEngine};

/// Decode a directory of real recordings, given `CALLMIND_TEST_AUDIO_DIR`.
///
/// The filenames used to be hard-coded, which committed real contact names and
/// a real phone number into the repository.
#[tokio::test]
#[ignore = "Set CALLMIND_TEST_AUDIO_DIR to a directory of real recordings"]
async fn test_decode_multiple_real_calls_from_volume() {
    let Some(dir) = std::env::var_os("CALLMIND_TEST_AUDIO_DIR") else {
        panic!("set CALLMIND_TEST_AUDIO_DIR to run this test");
    };

    let mut call_files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("CALLMIND_TEST_AUDIO_DIR is not readable")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_lowercase().as_str(), "m4a" | "wav" | "mp3"))
        })
        .collect();
    call_files.sort();
    call_files.truncate(4);
    assert!(!call_files.is_empty(), "no audio files in {dir:?}");

    for file_path in call_files {
        let p = file_path.as_path();

        println!("\n>>> Testing real call: {:?}", p.file_name().unwrap());
        let decoded = AudioDecoder::decode_file(p).expect("Failed to decode real file");
        println!(
            "    Decoded: sample_rate={}Hz, channels={}, duration={:.2}s",
            decoded.sample_rate,
            decoded.channels,
            decoded.duration_ms() as f64 / 1000.0
        );

        let mode = ChannelAnalyzer::analyze(&decoded);
        println!("    Channel mode: {mode:?}");

        let resampled = AudioResampler::resample_to_16k_mono(&decoded).expect("Resampling failed");
        let vad = EnergyVadEngine::default();
        let regions = vad.detect(&resampled).await.expect("VAD failed");
        println!("    VAD detected {} speech regions", regions.len());

        assert!(decoded.duration_ms() > 0);
        assert!(!regions.is_empty());
    }
}
