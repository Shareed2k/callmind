//! Does the eigengap estimate recover the real speaker count?
//!
//! Measured against labels rather than intuition:
//!
//! - `ONE_SPEAKER_FILES`: recordings known to hold one voice (a microphone test).
//! - `CALLMIND_TEST_AUDIO_DIR`: a two-party phone archive, so two is the label.
//!
//! Runs the real path -- VAD, then the ONNX speaker embeddings over speech only.
//! Earlier probes skipped VAD and so measured an input the pipeline never sees.
//!
//! `SILERO_ONNX` is optional; without it the energy detector is used, which is
//! what production does today.

use callmind_audio::{AudioDecoder, AudioResampler};
use callmind_diarization::onnx_extractor::OnnxSpeakerEmbeddingExtractor;
use callmind_diarization::spectral::estimate_speaker_count_with;
use callmind_vad::{EnergyVadEngine, VadEngine};

async fn windows_over_speech(
    extractor: &OnnxSpeakerEmbeddingExtractor,
    path: &str,
) -> Option<Vec<Vec<f32>>> {
    let audio = AudioDecoder::decode_file(path).ok()?;
    let mono = AudioResampler::resample_to_16k_mono(&audio).ok()?;
    let regions = EnergyVadEngine::default().detect(&mono).await.ok()?;
    let sr = mono.sample_rate as usize;
    let win = (sr * 1500) / 1000;
    let hop = (sr * 750) / 1000;

    let mut out = Vec::new();
    for region in &regions {
        let from = (region.start_ms as usize * sr) / 1000;
        let to = ((region.end_ms as usize * sr) / 1000).min(mono.samples.len());
        let mut start = from;
        while start + win <= to {
            if let Ok(e) = extractor.extract_embedding(&mono.samples[start..start + win]) {
                out.push(e);
            }
            start += hop;
        }
        if out.len() >= 400 {
            break;
        }
    }
    Some(out)
}

#[tokio::test]
#[ignore = "needs the ONNX model and labelled audio"]
async fn eigengap_recovers_the_labelled_count() {
    // Cargo runs an integration test from the crate directory, not the
    // workspace root.
    let model = [
        "models/diarization/speaker_embedding.onnx",
        "../../models/diarization/speaker_embedding.onnx",
    ]
    .into_iter()
    .map(std::path::PathBuf::from)
    .find(|p| p.exists())
    .expect("speaker_embedding.onnx must be downloaded");
    let extractor = OnnxSpeakerEmbeddingExtractor::load(&model).expect("ONNX model loads");

    // Embedding is the expensive part, so every file is embedded once and the
    // sweep runs over the stored windows.
    let mut labelled: Vec<(usize, Vec<Vec<f32>>)> = Vec::new();

    if let Ok(files) = std::env::var("ONE_SPEAKER_FILES") {
        for path in files.split(',').filter(|s| !s.is_empty()) {
            let Some(embeddings) = windows_over_speech(&extractor, path).await else {
                continue;
            };
            println!("one-speaker file: {} windows", embeddings.len());
            labelled.push((1, embeddings));
        }
    }

    if let Ok(dir) = std::env::var("CALLMIND_TEST_AUDIO_DIR") {
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .expect("audio dir")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "m4a"))
            .collect();
        files.sort_by_key(|p| p.metadata().map(|m| m.len()).unwrap_or(u64::MAX));
        // Mid-length and up: a one-second missed call has nothing to cluster.
        let take: usize = std::env::var("ACCURACY_FILES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        let mid = files.len() / 2;
        let stride = ((files.len() - mid) / take.max(1)).max(1);

        for path in files.into_iter().skip(mid).step_by(stride).take(take) {
            let p = path.to_string_lossy().to_string();
            let Some(embeddings) = windows_over_speech(&extractor, &p).await else {
                continue;
            };
            if embeddings.len() < 8 {
                continue;
            }
            println!("phone call: {} windows", embeddings.len());
            labelled.push((2, embeddings));
        }
    }

    println!("\nlabelled recordings: {}", labelled.len());
    println!(
        "\n{:<6} {:>10} {:>10} {:>28}",
        "keep", "k=1 right", "k=2 right", "estimates for the k=2 set"
    );
    // Per-file detail at the chosen operating point: do the misses cluster at
    // low window counts? If they do, a short recording can fall back to the prior
    // rather than to a bad estimate.
    println!("\n--- per file at keep=0.15 ---");
    println!(
        "{:>8} {:>7} {:>9} {:>7}",
        "windows", "label", "estimate", "ok"
    );
    for (label, embeddings) in &labelled {
        let got = estimate_speaker_count_with(embeddings, 6, 0.15).speakers;
        println!(
            "{:>8} {:>7} {:>9} {:>7}",
            embeddings.len(),
            label,
            got,
            if got == *label { "yes" } else { "NO" }
        );
    }

    for keep in [0.08_f32, 0.15] {
        let mut right_one = 0;
        let mut total_one = 0;
        let mut right_two = 0;
        let mut total_two = 0;
        let mut histogram: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        for (label, embeddings) in &labelled {
            let got = estimate_speaker_count_with(embeddings, 6, keep).speakers;
            if *label == 1 {
                total_one += 1;
                right_one += usize::from(got == 1);
            } else {
                total_two += 1;
                *histogram.entry(got).or_insert(0) += 1;
                right_two += usize::from(got == 2);
            }
        }
        println!(
            "{keep:<6.2} {:>10} {:>10} {:>28}",
            format!("{right_one}/{total_one}"),
            format!("{right_two}/{total_two}"),
            format!("{histogram:?}")
        );
    }
}
