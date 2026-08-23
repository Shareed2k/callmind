//! Does a longer embedding window recover the speaker count better?
//!
//! 1.5 s is short for a speaker embedding -- WeSpeaker is normally run on 2-4 s
//! -- and a noisier embedding is a plausible reason the count estimate misses.
//! This sweeps the window length against the labelled set, so the choice comes
//! from measurement rather than from what the pipeline happens to use today.

use callmind_audio::{AudioBuffer, AudioDecoder, AudioResampler};
use callmind_diarization::onnx_extractor::OnnxSpeakerEmbeddingExtractor;
use callmind_diarization::spectral::estimate_speaker_count_with;
use callmind_vad::{EnergyVadEngine, SpeechRegion, VadEngine};

fn windows(
    extractor: &OnnxSpeakerEmbeddingExtractor,
    mono: &AudioBuffer,
    regions: &[SpeechRegion],
    window_ms: usize,
) -> Vec<Vec<f32>> {
    let sr = mono.sample_rate as usize;
    let win = (sr * window_ms) / 1000;
    // Half-overlap, so the window count scales with the window length rather
    // than staying fixed and changing only the content.
    let hop = win / 2;
    let mut out = Vec::new();
    for region in regions {
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
    out
}

#[tokio::test]
#[ignore = "needs the ONNX model and labelled audio"]
async fn sweep_window_length() {
    let model = [
        "models/diarization/speaker_embedding.onnx",
        "../../models/diarization/speaker_embedding.onnx",
    ]
    .into_iter()
    .map(std::path::PathBuf::from)
    .find(|p| p.exists())
    .expect("speaker_embedding.onnx");
    let extractor = OnnxSpeakerEmbeddingExtractor::load(&model).expect("ONNX model loads");

    let mut corpus: Vec<(usize, String)> = Vec::new();
    if let Ok(files) = std::env::var("ONE_SPEAKER_FILES") {
        corpus.extend(
            files
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|p| (1, p.to_string())),
        );
    }
    if let Ok(dir) = std::env::var("CALLMIND_TEST_AUDIO_DIR") {
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .expect("audio dir")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "m4a"))
            .collect();
        files.sort_by_key(|p| p.metadata().map(|m| m.len()).unwrap_or(u64::MAX));
        let take: usize = std::env::var("ACCURACY_FILES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(12);
        let mid = files.len() / 2;
        let stride = ((files.len() - mid) / take.max(1)).max(1);
        corpus.extend(
            files
                .into_iter()
                .skip(mid)
                .step_by(stride)
                .take(take)
                .map(|p| (2, p.to_string_lossy().to_string())),
        );
    }

    // Decode and detect speech once; only the embedding window varies.
    let mut prepared: Vec<(usize, AudioBuffer, Vec<SpeechRegion>)> = Vec::new();
    for (label, path) in &corpus {
        let Ok(audio) = AudioDecoder::decode_file(path) else {
            continue;
        };
        let Ok(mono) = AudioResampler::resample_to_16k_mono(&audio) else {
            continue;
        };
        let regions = EnergyVadEngine::default()
            .detect(&mono)
            .await
            .unwrap_or_default();
        prepared.push((*label, mono, regions));
    }
    println!("labelled recordings: {}", prepared.len());

    // Per-file detail at the best window length, including the second-smallest
    // Laplacian eigenvalue. Eigengap always finds *some* gap, so it cannot answer
    // "one cluster"; algebraic connectivity can -- it is near zero when the graph
    // splits and large when it does not.
    println!("\n--- per file at 2000 ms ---");
    println!(
        "{:>7} {:>9} {:>10} {:>10}",
        "label", "windows", "estimate", "lambda2"
    );
    for (label, mono, regions) in &prepared {
        let embeddings = windows(&extractor, mono, regions, 2000);
        if embeddings.len() < 4 {
            println!("{label:>7} {:>9} {:>10} {:>10}", embeddings.len(), "-", "-");
            continue;
        }
        let est = estimate_speaker_count_with(&embeddings, 6, 0.08);
        let lambda2 = est.eigenvalues.get(1).copied().unwrap_or(f32::NAN);
        println!(
            "{label:>7} {:>9} {:>10} {:>10.4}",
            embeddings.len(),
            est.speakers,
            lambda2
        );
    }

    println!(
        "\n{:<10} {:>8} {:>8} {:>10} {:>24}",
        "window", "k=1", "k=2", "windows/f", "estimates for k=2"
    );
    for window_ms in [1500usize, 2000, 3000, 4000] {
        let mut right1 = 0;
        let mut total1 = 0;
        let mut right2 = 0;
        let mut total2 = 0;
        let mut counts = Vec::new();
        let mut histogram: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();

        for (label, mono, regions) in &prepared {
            let embeddings = windows(&extractor, mono, regions, window_ms);
            counts.push(embeddings.len());
            let estimate = if embeddings.len() < 4 {
                1
            } else {
                estimate_speaker_count_with(&embeddings, 6, 0.08).speakers
            };
            if *label == 1 {
                total1 += 1;
                right1 += usize::from(estimate == 1);
            } else {
                total2 += 1;
                *histogram.entry(estimate).or_insert(0) += 1;
                right2 += usize::from(estimate == 2);
            }
        }

        counts.sort_unstable();
        println!(
            "{:<10} {:>8} {:>8} {:>10} {:>24}",
            format!("{window_ms}ms"),
            format!("{right1}/{total1}"),
            format!("{right2}/{total2}"),
            counts.get(counts.len() / 2).copied().unwrap_or(0),
            format!("{histogram:?}")
        );
    }
}
