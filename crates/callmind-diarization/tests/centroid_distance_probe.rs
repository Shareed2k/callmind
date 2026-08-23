//! Diagnostic: how far apart are cluster centroids for one voice versus two?
//!
//! Not an assertion about quality -- it prints the numbers a threshold has to
//! separate, for both feature extractors.

use callmind_diarization::ahc::AgglomerativeClustering;
use callmind_diarization::features::AcousticFeatureExtractor;
use std::f32::consts::PI;

fn tone(freq: f32, secs: f32, sample_rate: u32) -> Vec<f32> {
    let n = (secs * sample_rate as f32) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            // A couple of harmonics, so it is not a bare sinusoid.
            0.6 * (2.0 * PI * freq * t).sin() + 0.2 * (2.0 * PI * freq * 2.0 * t).sin()
        })
        .collect()
}

fn windows(samples: &[f32], sample_rate: u32) -> Vec<Vec<f32>> {
    let win = (sample_rate as usize * 800) / 1000;
    let hop = (sample_rate as usize * 400) / 1000;
    let mut out = Vec::new();
    let mut start = 0;
    while start + win <= samples.len() {
        out.push(AcousticFeatureExtractor::extract_embedding(
            &samples[start..start + win],
            sample_rate,
        ));
        start += hop;
    }
    out
}

fn centroid_gap(embeddings: &[Vec<f32>], labels: &[usize]) -> f32 {
    let mut sums = std::collections::BTreeMap::new();
    for (&l, e) in labels.iter().zip(embeddings) {
        let entry = sums
            .entry(l)
            .or_insert_with(|| (vec![0.0f32; e.len()], 0usize));
        for (a, &v) in entry.0.iter_mut().zip(e) {
            *a += v;
        }
        entry.1 += 1;
    }
    let cs: Vec<Vec<f32>> = sums
        .values()
        .map(|(s, c)| s.iter().map(|v| v / *c as f32).collect())
        .collect();
    if cs.len() < 2 {
        return f32::NAN;
    }
    AcousticFeatureExtractor::cosine_distance(&cs[0], &cs[1])
}

#[test]
fn print_centroid_gaps() {
    let sr = 16_000;

    // One voice, held steady.
    let one = tone(150.0, 4.0, sr);
    // One voice with natural pitch drift, which is the realistic single-speaker
    // case: a microphone test is not a constant tone.
    let mut drifting = tone(140.0, 2.0, sr);
    drifting.extend(tone(165.0, 2.0, sr));
    // Two clearly different voices.
    let mut two = tone(120.0, 2.0, sr);
    two.extend(tone(280.0, 2.0, sr));

    for (name, samples) in [
        ("one voice, steady", one),
        ("one voice, drifting", drifting),
        ("two voices, 120 vs 280 Hz", two),
    ] {
        let embeddings = windows(&samples, sr);
        // Forced into two clusters, which is what the old default did.
        let forced = AgglomerativeClustering::new(0.35, Some(2)).cluster(&embeddings);
        let gap = centroid_gap(&embeddings, &forced);
        let sizes = {
            let mut m = std::collections::BTreeMap::new();
            for &l in &forced {
                *m.entry(l).or_insert(0) += 1;
            }
            m
        };
        // And what the threshold alone decides.
        let free = AgglomerativeClustering::new(0.35, None).cluster(&embeddings);
        let free_count = free.iter().collect::<std::collections::BTreeSet<_>>().len();
        println!(
            "{name:28} windows={:3} forced-2 centroid gap={gap:.4} sizes={sizes:?} threshold-only clusters={free_count}",
            embeddings.len()
        );
    }
}
