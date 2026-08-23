//! Does the clustering recover the right number of speakers?
//!
//! Two cases with hard ground truth, built from real recordings:
//!
//! - **two speakers**: audio from two *different* recordings concatenated. Two
//!   different phone numbers are two different people, so the answer is at least
//!   two and a result of one is a definite failure.
//! - **one speaker**: a single contiguous stretch from one recording, short
//!   enough (6 s) that it is one person talking. Weaker ground truth than the
//!   above, so it is reported rather than asserted.
//!
//! Prints the recovered count across candidate thresholds, so the value is
//! chosen from measurement rather than taste.
//!
//! `CALLMIND_TEST_AUDIO_DIR=... cargo test -p callmind-diarization --test
//! speaker_count_probe -- --ignored --nocapture`

use callmind_audio::{AudioBuffer, AudioDecoder, AudioResampler};
use callmind_diarization::ahc::AgglomerativeClustering;
use callmind_diarization::onnx_extractor::OnnxSpeakerEmbeddingExtractor;
use kodama::Method;

fn embed(
    extractor: &OnnxSpeakerEmbeddingExtractor,
    samples: &[f32],
    sample_rate: usize,
) -> Vec<Vec<f32>> {
    let win = (sample_rate * 1500) / 1000;
    let hop = (sample_rate * 750) / 1000;
    let mut out = Vec::new();
    let mut start = 0;
    while start + win <= samples.len() {
        if let Ok(e) = extractor.extract_embedding(&samples[start..start + win]) {
            out.push(e);
        }
        start += hop;
    }
    out
}

fn count(labels: &[usize]) -> usize {
    labels
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

#[test]
#[ignore = "needs the ONNX model and real recordings"]
fn probe_recovered_speaker_count() {
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
    let mid = files.len() / 2;
    let picked: Vec<_> = files.into_iter().skip(mid).take(4).collect();

    // 16 kHz mono, a 6-second stretch from a quarter into each recording.
    let clips: Vec<Vec<f32>> = picked
        .iter()
        .filter_map(|p| {
            let audio = AudioDecoder::decode_file(p).ok()?;
            let mono: AudioBuffer = AudioResampler::resample_to_16k_mono(&audio).ok()?;
            let sr = mono.sample_rate as usize;
            let from = mono.samples.len() / 4;
            let to = (from + sr * 6).min(mono.samples.len());
            (to > from + sr * 3).then(|| mono.samples[from..to].to_vec())
        })
        .collect();
    assert!(clips.len() >= 2, "need at least two usable recordings");
    println!("clips: {}\n", clips.len());

    let thresholds = [0.35_f32, 0.45, 0.55, 0.60, 0.65, 0.70, 0.75, 0.85];
    let mut header = String::new();
    for t in thresholds {
        use std::fmt::Write as _;
        let _ = write!(header, "{t:>5.2}");
    }
    println!("{:<28} {header}", "case");

    let report = |name: String, samples: &[f32], method: Method| {
        let embeddings = embed(&extractor, samples, 16_000);
        if embeddings.len() < 3 {
            println!("{name:<28} (too few windows: {})", embeddings.len());
            return;
        }
        let mut counts = String::new();
        for &t in &thresholds {
            use std::fmt::Write as _;
            let labels = AgglomerativeClustering::with_method(t, None, method).cluster(&embeddings);
            let _ = write!(counts, "{:>5}", count(&labels));
        }
        println!("{name:<28}{counts}  (windows={})", embeddings.len());
    };

    for (label, method) in [("COMPLETE", Method::Complete), ("AVERAGE", Method::Average)] {
        println!("\n--- linkage: {label} ---");
        for (i, clip) in clips.iter().enumerate() {
            report(format!("1 speaker  (clip #{i})"), clip, method);
        }
        for i in 0..clips.len() {
            for j in (i + 1)..clips.len() {
                let mut joined = clips[i].clone();
                joined.extend_from_slice(&clips[j]);
                report(format!("2 speakers (#{i} + #{j})"), &joined, method);
            }
        }
    }
}
