//! Diagnostic: centroid separation in the ONNX speaker-embedding space.
//!
//! The threshold that decides how many speakers a call has needs to be set from
//! measured distances, not guessed. This prints, for each real recording:
//!
//! - the centroid gap when the whole call is forced into two clusters (two
//!   speakers, for a two-person call), and
//! - the centroid gap when only the *majority* cluster's windows are forced into
//!   two (one voice, split against its will -- the exact failure being fixed).
//!
//! Ignored by default: needs the ONNX model and real audio.
//! `CALLMIND_TEST_AUDIO_DIR=... cargo test -p callmind-diarization --test
//! onnx_centroid_probe -- --ignored --nocapture`

use callmind_audio::{AudioDecoder, AudioResampler};
use callmind_diarization::ahc::AgglomerativeClustering;
use callmind_diarization::features::AcousticFeatureExtractor;
use callmind_diarization::onnx_extractor::OnnxSpeakerEmbeddingExtractor;

fn centroids(embeddings: &[Vec<f32>], labels: &[usize]) -> Vec<Vec<f32>> {
    let mut sums: std::collections::BTreeMap<usize, (Vec<f32>, usize)> =
        std::collections::BTreeMap::new();
    for (&l, e) in labels.iter().zip(embeddings) {
        let entry = sums
            .entry(l)
            .or_insert_with(|| (vec![0.0f32; e.len()], 0usize));
        for (a, &v) in entry.0.iter_mut().zip(e) {
            *a += v;
        }
        entry.1 += 1;
    }
    sums.values()
        .map(|(s, c)| s.iter().map(|v| v / *c as f32).collect())
        .collect()
}

fn forced_two_gap(embeddings: &[Vec<f32>]) -> Option<(f32, Vec<usize>)> {
    if embeddings.len() < 4 {
        return None;
    }
    let labels = AgglomerativeClustering::new(1.0, Some(2)).cluster(embeddings);
    let cs = centroids(embeddings, &labels);
    if cs.len() < 2 {
        return None;
    }
    Some((
        AcousticFeatureExtractor::cosine_distance(&cs[0], &cs[1]),
        labels,
    ))
}

#[test]
#[ignore = "needs the ONNX model and real recordings"]
fn probe_onnx_centroid_separation() {
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

    let take: usize = std::env::var("PROBE_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);

    // Sampled across the size distribution rather than from one end: the
    // smallest files in a real archive are one-second missed calls with too few
    // windows to cluster at all.
    let stride = (files.len() / (take + 1)).max(1);
    let files: Vec<_> = files.into_iter().step_by(stride).rev().collect();

    println!(
        "{:<6} {:>7} {:>12} {:>12} {:>8}",
        "file", "windows", "two-speaker", "one-voice", "ratio"
    );
    let mut two_speaker = Vec::new();
    let mut one_voice = Vec::new();

    for (i, path) in files.iter().take(take).enumerate() {
        let audio = match AudioDecoder::decode_file(path) {
            Ok(a) => a,
            Err(e) => {
                println!("#{i} decode failed: {e}");
                continue;
            }
        };
        let mono = match AudioResampler::resample_to_16k_mono(&audio) {
            Ok(m) => m,
            Err(e) => {
                println!("#{i} resample failed: {e}");
                continue;
            }
        };
        let sr = mono.sample_rate as usize;
        let win = (sr * 800) / 1000;
        let hop = (sr * 400) / 1000;

        let mut embeddings = Vec::new();
        let mut start = 0;
        while start + win <= mono.samples.len() {
            if let Ok(e) = extractor.extract_embedding(&mono.samples[start..start + win]) {
                embeddings.push(e);
            }
            start += hop;
            // Long calls are not needed to characterise the distance scale.
            if embeddings.len() >= 300 {
                break;
            }
        }

        let Some((whole_gap, labels)) = forced_two_gap(&embeddings) else {
            println!("#{i} only {} windows / embeddings", embeddings.len());
            continue;
        };

        // The larger cluster is the closest thing to "one voice" available from a
        // real recording without hand labels.
        let majority = {
            let mut counts = std::collections::BTreeMap::new();
            for &l in &labels {
                *counts.entry(l).or_insert(0usize) += 1;
            }
            counts.into_iter().max_by_key(|&(_, c)| c).map(|(l, _)| l)
        };
        let single: Vec<Vec<f32>> = embeddings
            .iter()
            .zip(&labels)
            .filter(|&(_, &l)| Some(l) == majority)
            .map(|(e, _)| e.clone())
            .collect();
        let Some((single_gap, _)) = forced_two_gap(&single) else {
            println!("#{i} majority cluster too small: {}", single.len());
            continue;
        };

        println!(
            "{:<6} {:>7} {:>12.4} {:>12.4} {:>8.1}x",
            format!("#{i}"),
            embeddings.len(),
            whole_gap,
            single_gap,
            whole_gap / single_gap.max(1e-6)
        );
        two_speaker.push(whole_gap);
        one_voice.push(single_gap);
    }

    let stat = |v: &mut Vec<f32>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (v.first().copied(), v.last().copied())
    };
    let (two_min, two_max) = stat(&mut two_speaker);
    let (one_min, one_max) = stat(&mut one_voice);
    println!("\ntwo-speaker gaps: min={two_min:?} max={two_max:?}");
    println!("one-voice   gaps: min={one_min:?} max={one_max:?}");
    println!("=> a usable threshold must sit above {one_max:?} and below {two_min:?}");
}
