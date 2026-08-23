//! Is the speaker-embedding extractor actually discriminative?
//!
//! Before any threshold can be chosen, the embedding space has to separate
//! people at all. This measures the only thing that matters for that:
//!
//! - **within** a single recording: distances between windows, which mix the same
//!   speaker with the other party, and
//! - **across** recordings: distances between windows of *different* phone
//!   numbers, which are almost certainly different people.
//!
//! A working extractor puts "across" clearly above "within". If the two overlap,
//! the embeddings are the problem and no clustering threshold can rescue them.
//!
//! `CALLMIND_TEST_AUDIO_DIR=... cargo test -p callmind-diarization --test
//! embedding_sanity_probe -- --ignored --nocapture`

use callmind_audio::{AudioDecoder, AudioResampler};
use callmind_diarization::features::AcousticFeatureExtractor;
use callmind_diarization::onnx_extractor::OnnxSpeakerEmbeddingExtractor;

fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return f32::NAN;
    }
    let idx = ((sorted.len() - 1) as f32 * p).round() as usize;
    sorted[idx]
}

fn summarise(name: &str, mut values: Vec<f32>) {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "{name:<10} n={:<6} p05={:.3} p25={:.3} median={:.3} p75={:.3} p95={:.3}",
        values.len(),
        percentile(&values, 0.05),
        percentile(&values, 0.25),
        percentile(&values, 0.50),
        percentile(&values, 0.75),
        percentile(&values, 0.95),
    );
}

#[test]
#[ignore = "needs the ONNX model and real recordings"]
fn probe_embedding_discrimination() {
    let model = std::path::Path::new("models/diarization/speaker_embedding.onnx");
    let model = if model.exists() {
        model.to_path_buf()
    } else {
        std::path::Path::new("../../models/diarization/speaker_embedding.onnx").to_path_buf()
    };
    let extractor = OnnxSpeakerEmbeddingExtractor::load(&model).expect("ONNX model loads");

    let dir = std::env::var("CALLMIND_TEST_AUDIO_DIR").expect("CALLMIND_TEST_AUDIO_DIR");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("audio dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "m4a"))
        .collect();
    files.sort_by_key(|p| p.metadata().map(|m| m.len()).unwrap_or(u64::MAX));
    // Mid-length recordings: long enough to window, short enough to decode fast.
    let mid = files.len() / 2;
    let files: Vec<_> = files.into_iter().skip(mid).take(6).collect();

    // Windows per recording, from the middle of the audio where speech is likely.
    let per_file: Vec<Vec<Vec<f32>>> = files
        .iter()
        .filter_map(|path| {
            let audio = AudioDecoder::decode_file(path).ok()?;
            let mono = AudioResampler::resample_to_16k_mono(&audio).ok()?;
            let sr = mono.sample_rate as usize;
            let win = (sr * 1500) / 1000;
            let hop = (sr * 750) / 1000;
            let mut out = Vec::new();
            let mut start = mono.samples.len() / 4;
            while start + win <= mono.samples.len() && out.len() < 20 {
                if let Ok(e) = extractor.extract_embedding(&mono.samples[start..start + win]) {
                    out.push(e);
                }
                start += hop;
            }
            (out.len() >= 4).then_some(out)
        })
        .collect();

    println!("recordings usable: {}", per_file.len());
    assert!(per_file.len() >= 2, "need at least two recordings");

    // "Within one recording" is the wrong control: a two-person call contains
    // cross-speaker pairs, which *should* be far apart. Adjacent windows are the
    // usable proxy for the same speaker -- they are 0.75 s apart and people speak
    // in turns considerably longer than that, so the large majority of adjacent
    // pairs are one voice.
    let mut adjacent = Vec::new();
    let mut within = Vec::new();
    for windows in &per_file {
        for i in 0..windows.len() {
            if i + 1 < windows.len() {
                adjacent.push(AcousticFeatureExtractor::cosine_distance(
                    &windows[i],
                    &windows[i + 1],
                ));
            }
            for j in (i + 1)..windows.len() {
                within.push(AcousticFeatureExtractor::cosine_distance(
                    &windows[i],
                    &windows[j],
                ));
            }
        }
    }

    let mut across = Vec::new();
    for a in 0..per_file.len() {
        for b in (a + 1)..per_file.len() {
            for wa in &per_file[a] {
                for wb in &per_file[b] {
                    across.push(AcousticFeatureExtractor::cosine_distance(wa, wb));
                }
            }
        }
    }

    summarise("adjacent", adjacent.clone());
    summarise("within", within.clone());
    summarise("across", across.clone());

    // Also: how much of the embedding is actually varying? A near-constant
    // output would explain tiny distances regardless of who is speaking.
    let flat = &per_file[0][0];
    let mean = flat.iter().sum::<f32>() / flat.len() as f32;
    let var = flat.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / flat.len() as f32;
    println!(
        "\nembedding dim={} mean={mean:.4} sd={:.4} min={:.4} max={:.4}",
        flat.len(),
        var.sqrt(),
        flat.iter().copied().fold(f32::INFINITY, f32::min),
        flat.iter().copied().fold(f32::NEG_INFINITY, f32::max),
    );

    let mut adjacent = adjacent.clone();
    adjacent.sort_by(|a, b| a.partial_cmp(b).unwrap());
    across.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let same_high = percentile(&adjacent, 0.75);
    let diff_low = percentile(&across, 0.25);
    println!("\nsame speaker (adjacent) p75={same_high:.3} vs different people p25={diff_low:.3}");
    println!(
        "verdict: {}",
        if diff_low > same_high {
            format!(
                "separable -- a threshold between {same_high:.3} and {diff_low:.3} splits the bulk of both"
            )
        } else {
            "overlapping even on the bulk of the distributions".to_string()
        }
    );
}
