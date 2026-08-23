//! Estimating *how many* speakers are present, rather than being told.
//!
//! **Superseded.** [`crate::pyannote`] answers this far better -- 4/4 monologues
//! and 23/24 two-party calls against this module's 4/4 and 10/14, and without any
//! threshold to tune. This is kept because the measurements behind it are worth
//! not repeating, and because it needs no extra model. Do not wire it into the
//! pipeline ahead of segmentation.
//!
//! Threshold-based clustering cannot answer this: the threshold that stops a
//! monologue from splitting is above the distance at which two real speakers sit,
//! measured on real recordings in `tests/onnx_centroid_probe.rs`. Any single
//! cutoff therefore either splits one voice or merges two.
//!
//! The eigengap heuristic sidesteps that because it is **scale-free**. It reads
//! the number of clusters off the *shape* of the affinity structure -- the
//! largest jump between consecutive Laplacian eigenvalues -- rather than off an
//! absolute distance. That is the standard approach for unknown speaker counts
//! (Google's "Speaker Diarization with LSTM" describes the same refinement and
//! eigengap pipeline).
//!
//! The affinity refinement steps matter as much as the eigengap itself: raw
//! cosine similarity between short windows is noisy, and the row-thresholding
//! plus diffusion below is what turns it into something with visible block
//! structure.

use crate::features::AcousticFeatureExtractor;

/// Everything the estimator decided, so a caller can log why.
#[derive(Debug, Clone)]
pub struct SpeakerCountEstimate {
    /// The chosen number of speakers.
    pub speakers: usize,
    /// Laplacian eigenvalues, ascending, as far as they were computed.
    pub eigenvalues: Vec<f32>,
    /// The gap that won.
    pub gap: f32,
}

/// Affinity matrix from embeddings: cosine similarity, zero on the diagonal.
fn affinity(embeddings: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let n = embeddings.len();
    let mut a = vec![vec![0.0f32; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            // Similarity, not distance: 1 - cosine distance, floored at zero so
            // opposing vectors do not contribute negative weight.
            let s = (1.0
                - AcousticFeatureExtractor::cosine_distance(&embeddings[i], &embeddings[j]))
            .max(0.0);
            a[i][j] = s;
            a[j][i] = s;
        }
    }
    a
}

/// Refine a raw affinity matrix into one with usable block structure.
///
/// Row thresholding keeps only each window's strongest neighbours, which removes
/// the weak-but-nonzero similarity that every window has to every other and that
/// otherwise smears the blocks together. Symmetrisation and one diffusion step
/// then propagate "my neighbour's neighbour is probably my speaker".
fn refine(mut a: Vec<Vec<f32>>, keep_fraction: f32) -> Vec<Vec<f32>> {
    let n = a.len();
    if n < 2 {
        return a;
    }

    // Row-wise: keep the top `keep_fraction` of each row, zero the rest.
    let keep = ((n as f32 * keep_fraction).round() as usize).clamp(1, n);
    for row in &mut a {
        let mut sorted = row.clone();
        sorted.sort_by(|x, y| y.partial_cmp(x).unwrap_or(std::cmp::Ordering::Equal));
        let cutoff = sorted[keep - 1];
        for v in row.iter_mut() {
            if *v < cutoff {
                *v = 0.0;
            }
        }
    }

    // Symmetrise by max: thresholding is per row, so it is not symmetric.
    for i in 0..n {
        for j in (i + 1)..n {
            let m = a[i][j].max(a[j][i]);
            a[i][j] = m;
            a[j][i] = m;
        }
    }

    // One diffusion step, `Y = A * A`, which is symmetric because A is.
    let mut y = vec![vec![0.0f32; n]; n];
    for i in 0..n {
        for j in i..n {
            let mut acc = 0.0;
            for k in 0..n {
                acc += a[i][k] * a[k][j];
            }
            y[i][j] = acc;
            y[j][i] = acc;
        }
    }

    // Row-max normalisation, so no window dominates by loudness alone.
    for row in &mut y {
        let max = row.iter().copied().fold(0.0f32, f32::max);
        if max > 1e-6 {
            for v in row.iter_mut() {
                *v /= max;
            }
        }
    }
    y
}

/// Eigenvalues of a symmetric matrix, ascending, by cyclic Jacobi rotations.
///
/// Written out rather than pulled in: the workspace has no linear-algebra
/// dependency, and adding one for eigenvalues of a few-hundred-square matrix is
/// a poor trade in a project whose build has already been the hard part. Jacobi
/// is O(n^3) but exact for symmetric input and needs no pivoting strategy.
fn symmetric_eigenvalues(mut m: Vec<Vec<f32>>) -> Vec<f32> {
    let n = m.len();
    if n == 0 {
        return Vec::new();
    }

    for _sweep in 0..64 {
        // Largest off-diagonal magnitude decides when to stop.
        let mut off = 0.0f32;
        for i in 0..n {
            for j in (i + 1)..n {
                off += m[i][j] * m[i][j];
            }
        }
        if off.sqrt() < 1e-6 {
            break;
        }

        for p in 0..n {
            for q in (p + 1)..n {
                if m[p][q].abs() < 1e-9 {
                    continue;
                }
                let theta = 0.5 * (m[q][q] - m[p][p]) / m[p][q];
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;

                for k in 0..n {
                    let akp = m[k][p];
                    let akq = m[k][q];
                    m[k][p] = c * akp - s * akq;
                    m[k][q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let apk = m[p][k];
                    let aqk = m[q][k];
                    m[p][k] = c * apk - s * aqk;
                    m[q][k] = s * apk + c * aqk;
                }
            }
        }
    }

    let mut eigenvalues: Vec<f32> = (0..n).map(|i| m[i][i]).collect();
    eigenvalues.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    eigenvalues
}

/// Fraction of each affinity row kept by the pruning step.
///
/// The single parameter that matters, and it is not a distance threshold -- it is
/// "how many neighbours count", which is why the estimate stays scale-free.
/// Keeping too many smears the blocks into one and the estimate collapses to a
/// single speaker; keeping too few fragments them. Set from measurement against
/// labelled recordings (`tests/speaker_count_accuracy.rs`).
pub const DEFAULT_KEEP_FRACTION: f32 = 0.10;

/// Distance at which two embeddings stop being the same voice.
///
/// `pyannote/speaker-diarization-3.1` was tuned to 0.7046 driving the *same*
/// embedding model this project uses, and an independent measurement here put
/// same-speaker window pairs at p75 = 0.570 and different-people pairs at
/// p25 = 0.737 -- so the tuned value sits between the two distributions, as it
/// should. 0.65 measured marginally better on this corpus.
///
/// For contrast, the value this project used before was **0.35**, which is far
/// below where one voice already spreads to. Left to itself it produced up to 183
/// clusters on a two-party call; only a hard-coded "there are exactly two
/// speakers" concealed that.
pub const SPEAKER_DISTANCE_THRESHOLD: f32 = 0.65;

/// Smallest number of windows that counts as a participant.
///
/// pyannote's `min_cluster_size`. See
/// [`crate::ahc::AgglomerativeClustering::absorb_small_clusters`].
pub const MIN_CLUSTER_WINDOWS: usize = 12;

/// Embedding window the estimator was validated at, in milliseconds.
///
/// Not negotiable: at the 800 ms window used for turn assignment the same rule
/// scores 2/4 and 7/14 instead of 4/4 and 10/14. Two seconds is also the length
/// speaker-embedding models are normally run at.
pub const ESTIMATION_WINDOW_MS: usize = 2000;

/// Estimate the speaker count from window embeddings.
///
/// Two estimators, and the smaller answer wins. They fail in opposite
/// directions, measured against labelled recordings -- four monologues confirmed
/// by their owner and fourteen two-party phone calls:
///
/// | rule | monologues | two-party |
/// | :--- | ---: | ---: |
/// | threshold clustering with a minimum cluster size | 4/4 | 8/14 |
/// | eigengap alone | 2/4 | 11/14 |
/// | **the smaller of the two** | **4/4** | **10/14** |
///
/// The eigengap cannot answer "one": it looks for the largest jump between
/// consecutive eigenvalues and there is always some jump to find. Threshold
/// clustering can, but over-splits a long conversation. Taking the minimum lets
/// each cover the other's blind spot.
///
/// `max_speakers` caps the search.
#[must_use]
pub fn estimate_speaker_count(
    embeddings: &[Vec<f32>],
    max_speakers: usize,
) -> SpeakerCountEstimate {
    let mut estimate = estimate_speaker_count_with(embeddings, max_speakers, DEFAULT_KEEP_FRACTION);
    if embeddings.len() < 4 {
        return estimate;
    }

    let clustered = crate::ahc::AgglomerativeClustering::with_method(
        SPEAKER_DISTANCE_THRESHOLD,
        None,
        kodama::Method::Average,
    )
    .cluster(embeddings);
    let absorbed = crate::ahc::AgglomerativeClustering::absorb_small_clusters(
        &clustered,
        embeddings,
        MIN_CLUSTER_WINDOWS,
    );
    let by_threshold = absorbed
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();

    estimate.speakers = estimate.speakers.min(by_threshold).clamp(1, max_speakers);
    estimate
}

/// Same, with the pruning fraction supplied -- used by the parameter sweep.
#[must_use]
pub fn estimate_speaker_count_with(
    embeddings: &[Vec<f32>],
    max_speakers: usize,
    keep_fraction: f32,
) -> SpeakerCountEstimate {
    let n = embeddings.len();
    if n < 4 {
        return SpeakerCountEstimate {
            speakers: 1,
            eigenvalues: Vec::new(),
            gap: 0.0,
        };
    }

    // Subsampled uniformly above a few hundred windows: Jacobi is cubic, and the
    // eigengap describes structure that a uniform sample preserves.
    const MAX_WINDOWS: usize = 256;
    let sampled: Vec<Vec<f32>> = if n > MAX_WINDOWS {
        let stride = n.div_ceil(MAX_WINDOWS);
        embeddings.iter().step_by(stride).cloned().collect()
    } else {
        embeddings.to_vec()
    };

    let refined = refine(affinity(&sampled), keep_fraction);
    let size = refined.len();

    // Normalised Laplacian, `I - D^-1/2 A D^-1/2`, whose eigenvalues live in
    // [0, 2] regardless of how the affinities are scaled -- which is the property
    // that makes this threshold-free.
    let degrees: Vec<f32> = refined
        .iter()
        .map(|row| row.iter().sum::<f32>().max(1e-6))
        .collect();
    let mut laplacian = vec![vec![0.0f32; size]; size];
    for i in 0..size {
        for j in 0..size {
            let norm = (degrees[i] * degrees[j]).sqrt();
            laplacian[i][j] = if i == j {
                1.0 - refined[i][j] / norm
            } else {
                -refined[i][j] / norm
            };
        }
    }

    let eigenvalues = symmetric_eigenvalues(laplacian);
    let ceiling = max_speakers.min(size.saturating_sub(1)).max(1);

    // The largest jump between consecutive eigenvalues; `k` clusters show up as a
    // gap after the k-th smallest.
    let mut best = (1usize, 0.0f32);
    for k in 1..=ceiling {
        if k >= eigenvalues.len() {
            break;
        }
        let gap = eigenvalues[k] - eigenvalues[k - 1];
        if gap > best.1 {
            best = (k, gap);
        }
    }

    SpeakerCountEstimate {
        speakers: best.0,
        eigenvalues: eigenvalues.into_iter().take(ceiling + 2).collect(),
        gap: best.1,
    }
}

pub const MAX_COUNTING_WINDOWS: usize = 256;

/// Below this many counting windows the estimate should not be trusted.
///
/// Measured: a recording yielding 9 windows was estimated at 4 speakers when it
/// held 2. Twenty is the lowest count that answered correctly in the labelled
/// runs.
pub const MIN_WINDOWS_TO_ESTIMATE: usize = 20;

/// Embeddings over long windows, for estimating *how many* speakers there are.
///
/// Deliberately separate from the windows used to assign turns. Two seconds is
/// the length the estimator was validated at, and it is capped at
/// [`MAX_COUNTING_WINDOWS`] and spread uniformly across the speech, so the cost
/// does not grow with call length -- an hour-long call pays the same bounded
/// number of inferences as a five-minute one.
pub fn embed_for_counting(
    extractor: &crate::onnx_extractor::OnnxSpeakerEmbeddingExtractor,
    mono: &callmind_audio::AudioBuffer,
    regions: &[callmind_vad::SpeechRegion],
    sample_rate: usize,
) -> Vec<Vec<f32>> {
    let window = (sample_rate * ESTIMATION_WINDOW_MS) / 1000;
    let hop = window / 2;

    // Plan the windows first so they can be thinned before any inference runs.
    let mut planned: Vec<usize> = Vec::new();
    for region in regions {
        let from = (region.start_ms as usize * sample_rate) / 1000;
        let to = ((region.end_ms as usize * sample_rate) / 1000).min(mono.samples.len());
        let mut start = from;
        while start + window <= to {
            planned.push(start);
            start += hop;
        }
    }

    let stride = planned.len().div_ceil(MAX_COUNTING_WINDOWS).max(1);
    planned
        .into_iter()
        .step_by(stride)
        .filter_map(|start| {
            extractor
                .extract_embedding(&mono.samples[start..start + window])
                .ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Well-separated synthetic groups, where the answer is not in doubt.
    fn groups(count_per_group: usize, groups: usize, dim: usize) -> Vec<Vec<f32>> {
        let mut out = Vec::new();
        for g in 0..groups {
            for i in 0..count_per_group {
                let mut v = vec![0.0f32; dim];
                v[g] = 1.0;
                // A little jitter, so the affinity is not exactly rank-deficient.
                v[(g + 1 + i) % dim] += 0.05;
                out.push(v);
            }
        }
        out
    }

    #[test]
    fn recovers_obvious_group_counts() {
        for expected in 1..=4 {
            let embeddings = groups(8, expected, 16);
            let estimate = estimate_speaker_count(&embeddings, 8);
            assert_eq!(
                estimate.speakers, expected,
                "expected {expected}, eigenvalues {:?}",
                estimate.eigenvalues
            );
        }
    }

    #[test]
    fn degenerate_input_reports_one_speaker() {
        assert_eq!(estimate_speaker_count(&[], 8).speakers, 1);
        assert_eq!(estimate_speaker_count(&[vec![1.0, 0.0]], 8).speakers, 1);
        // Identical embeddings are one speaker, not many.
        let same = vec![vec![1.0f32, 0.0, 0.0]; 12];
        assert_eq!(estimate_speaker_count(&same, 8).speakers, 1);
    }

    #[test]
    fn jacobi_matches_a_known_spectrum() {
        // Diagonal matrix: eigenvalues are the diagonal.
        let m = vec![
            vec![3.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 2.0],
        ];
        let e = symmetric_eigenvalues(m);
        assert!((e[0] - 1.0).abs() < 1e-4, "{e:?}");
        assert!((e[1] - 2.0).abs() < 1e-4, "{e:?}");
        assert!((e[2] - 3.0).abs() < 1e-4, "{e:?}");

        // 2x2 with known closed form: [[2,1],[1,2]] has eigenvalues 1 and 3.
        let e = symmetric_eigenvalues(vec![vec![2.0, 1.0], vec![1.0, 2.0]]);
        assert!((e[0] - 1.0).abs() < 1e-4, "{e:?}");
        assert!((e[1] - 3.0).abs() < 1e-4, "{e:?}");
    }
}
