//! Does pyannote's own tuned configuration recover the speaker count?
//!
//! `pyannote/speaker-diarization-3.1` ships the parameters it was tuned with, and
//! it drives the *same* embedding model this project uses
//! (`wespeaker-voxceleb-resnet34-LM`). So the values are directly informative
//! rather than a guess:
//!
//! ```yaml
//! clustering: AgglomerativeClustering
//!   method: centroid
//!   threshold: 0.7045654963945799
//!   min_cluster_size: 12
//! ```
//!
//! Two things differ sharply from what this project does today: the threshold is
//! **0.70, not 0.35**, and clusters smaller than 12 observations are discarded.
//! That second rule is exactly the missing filter -- the phantom speaker seen in
//! practice was one word over 310 ms.
//!
//! Caveat worth stating: pyannote clusters one embedding per local speaker per
//! chunk, taken from its segmentation model, not per fixed window as here. So the
//! parameters are a strong reference, not a drop-in.

use callmind_audio::{AudioBuffer, AudioDecoder, AudioResampler};
use callmind_diarization::ahc::AgglomerativeClustering;
use callmind_diarization::features::AcousticFeatureExtractor;
use callmind_diarization::onnx_extractor::OnnxSpeakerEmbeddingExtractor;
use callmind_vad::{EnergyVadEngine, SpeechRegion, VadEngine};
use kodama::Method;

/// Drop clusters below `min_size`, reassigning their members to the nearest
/// surviving centroid. This is pyannote's `min_cluster_size`.
fn absorb_small_clusters(labels: &[usize], embeddings: &[Vec<f32>], min_size: usize) -> Vec<usize> {
    let mut counts: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    for &l in labels {
        *counts.entry(l).or_insert(0) += 1;
    }
    let survivors: Vec<usize> = counts
        .iter()
        .filter(|&(_, &c)| c >= min_size)
        .map(|(&l, _)| l)
        .collect();
    if survivors.is_empty() || survivors.len() == counts.len() {
        // Nothing to absorb, or nothing big enough to absorb into.
        return renumber(labels);
    }

    // Centroid per surviving cluster.
    let centroids: Vec<(usize, Vec<f32>)> = survivors
        .iter()
        .map(|&s| {
            let members: Vec<&Vec<f32>> = labels
                .iter()
                .zip(embeddings)
                .filter(|(l, _)| **l == s)
                .map(|(_, e)| e)
                .collect();
            let dim = members[0].len();
            let mut sum = vec![0.0f32; dim];
            for m in &members {
                for (a, v) in sum.iter_mut().zip(m.iter()) {
                    *a += v;
                }
            }
            let n = members.len() as f32;
            (s, sum.into_iter().map(|v| v / n).collect())
        })
        .collect();

    let reassigned: Vec<usize> = labels
        .iter()
        .zip(embeddings)
        .map(|(&l, e)| {
            if survivors.contains(&l) {
                return l;
            }
            centroids
                .iter()
                .min_by(|a, b| {
                    AcousticFeatureExtractor::cosine_distance(e, &a.1)
                        .partial_cmp(&AcousticFeatureExtractor::cosine_distance(e, &b.1))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map_or(l, |(s, _)| *s)
        })
        .collect();
    renumber(&reassigned)
}

fn renumber(labels: &[usize]) -> Vec<usize> {
    let mut map: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    labels
        .iter()
        .map(|&l| {
            let next = map.len();
            *map.entry(l).or_insert(next)
        })
        .collect()
}

fn count(labels: &[usize]) -> usize {
    labels
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

fn windows(
    extractor: &OnnxSpeakerEmbeddingExtractor,
    mono: &AudioBuffer,
    regions: &[SpeechRegion],
    window_ms: usize,
) -> Vec<Vec<f32>> {
    let sr = mono.sample_rate as usize;
    let win = (sr * window_ms) / 1000;
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
async fn pyannote_parameters_against_labels() {
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
            .unwrap_or(14);
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

    let mut prepared: Vec<(usize, Vec<Vec<f32>>)> = Vec::new();
    let mut at_800: Vec<(usize, Vec<Vec<f32>>)> = Vec::new();
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
        // Both the pipeline's current window and the one that measured best, so
        // the chosen parameters are validated at the length actually used.
        prepared.push((*label, windows(&extractor, &mono, &regions, 2000)));
        at_800.push((*label, windows(&extractor, &mono, &regions, 800)));
    }
    println!("labelled recordings: {}\n", prepared.len());

    // The two estimators fail in complementary ways: threshold clustering with a
    // minimum cluster size recognises a monologue, and the eigengap recovers the
    // count on a two-party call. So combining them is worth measuring, not just
    // picking the better one.
    println!("--- combination rules ---");
    println!("(window = 2000 ms unless the row says otherwise)");
    println!(
        "{:<34} {:>8} {:>8} {:>20}",
        "rule", "k=1", "k=2", "estimates for k=2"
    );
    let ahc_count = |embeddings: &Vec<Vec<f32>>| {
        if embeddings.len() < 4 {
            return 1;
        }
        let raw =
            AgglomerativeClustering::with_method(0.65, None, Method::Average).cluster(embeddings);
        count(&absorb_small_clusters(&raw, embeddings, 12))
    };
    let eigen_count = |embeddings: &Vec<Vec<f32>>| {
        if embeddings.len() < 4 {
            return 1;
        }
        callmind_diarization::spectral::estimate_speaker_count_with(embeddings, 6, 0.08).speakers
    };

    for (name, rule) in [
        (
            "ahc only (0.65, min 12)",
            Box::new(move |e: &Vec<Vec<f32>>| ahc_count(e)) as Box<dyn Fn(&Vec<Vec<f32>>) -> usize>,
        ),
        (
            "eigengap only",
            Box::new(move |e: &Vec<Vec<f32>>| eigen_count(e)),
        ),
        (
            "ahc decides 1, else eigengap",
            Box::new(
                move |e: &Vec<Vec<f32>>| {
                    if ahc_count(e) == 1 { 1 } else { eigen_count(e) }
                },
            ),
        ),
        (
            "min of the two",
            Box::new(move |e: &Vec<Vec<f32>>| ahc_count(e).min(eigen_count(e))),
        ),
        (
            "both must exceed 1",
            Box::new(move |e: &Vec<Vec<f32>>| {
                let (a, g) = (ahc_count(e), eigen_count(e));
                if a > 1 && g > 1 { g } else { 1 }
            }),
        ),
    ] {
        let mut right1 = 0;
        let mut total1 = 0;
        let mut right2 = 0;
        let mut total2 = 0;
        let mut histogram: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        for (label, embeddings) in &prepared {
            let got = rule(embeddings);
            if *label == 1 {
                total1 += 1;
                right1 += usize::from(got == 1);
            } else {
                total2 += 1;
                *histogram.entry(got).or_insert(0) += 1;
                right2 += usize::from(got == 2);
            }
        }
        println!(
            "{name:<34} {:>8} {:>8} {:>20}",
            format!("{right1}/{total1}"),
            format!("{right2}/{total2}"),
            format!("{histogram:?}")
        );
    }

    // The same rule at the window length the pipeline uses today.
    for (wlabel, set) in [("2000 ms", &prepared), ("800 ms", &at_800)] {
        let mut right1 = 0;
        let mut total1 = 0;
        let mut right2 = 0;
        let mut total2 = 0;
        let mut histogram: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        let mut counts = Vec::new();
        for (label, embeddings) in set {
            counts.push(embeddings.len());
            let got = ahc_count(embeddings).min(eigen_count(embeddings));
            if *label == 1 {
                total1 += 1;
                right1 += usize::from(got == 1);
            } else {
                total2 += 1;
                *histogram.entry(got).or_insert(0) += 1;
                right2 += usize::from(got == 2);
            }
        }
        counts.sort_unstable();
        println!(
            "{:<34} {:>8} {:>8} {:>20}  median windows {}",
            format!("min-of-two at {wlabel}"),
            format!("{right1}/{total1}"),
            format!("{right2}/{total2}"),
            format!("{histogram:?}"),
            counts.get(counts.len() / 2).copied().unwrap_or(0)
        );
    }

    println!(
        "\n{:<10} {:>9} {:>6} {:>8} {:>8} {:>22}",
        "method", "threshold", "min", "k=1", "k=2", "estimates for k=2"
    );
    for (mlabel, method) in [("centroid", Method::Centroid), ("average", Method::Average)] {
        for threshold in [0.35_f32, 0.55, 0.65, 0.7046, 0.80] {
            for min_size in [1usize, 6, 12] {
                let mut right1 = 0;
                let mut total1 = 0;
                let mut right2 = 0;
                let mut total2 = 0;
                let mut histogram: std::collections::BTreeMap<usize, usize> =
                    std::collections::BTreeMap::new();

                for (label, embeddings) in &prepared {
                    let estimate = if embeddings.len() < 4 {
                        1
                    } else {
                        let raw = AgglomerativeClustering::with_method(threshold, None, method)
                            .cluster(embeddings);
                        count(&absorb_small_clusters(&raw, embeddings, min_size))
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
                println!(
                    "{mlabel:<10} {threshold:>9.4} {min_size:>6} {:>8} {:>8} {:>22}",
                    format!("{right1}/{total1}"),
                    format!("{right2}/{total2}"),
                    format!("{histogram:?}")
                );
            }
        }
    }
}
