use crate::features::AcousticFeatureExtractor;
use kodama::{Method, linkage};
use std::collections::HashMap;

/// Agglomerative Hierarchical Clustering (AHC) with complete linkage and distance thresholding.
pub struct AgglomerativeClustering {
    pub distance_threshold: f32,
    pub target_clusters: Option<usize>,
    /// Linkage rule. Exposed so it can be compared against measurements rather
    /// than assumed -- see `tests/speaker_count_probe.rs`.
    pub method: Method,
}

impl Default for AgglomerativeClustering {
    fn default() -> Self {
        Self {
            distance_threshold: 0.45,
            target_clusters: None,
            method: Method::Average,
        }
    }
}

/// Union-find root lookup with path compression.
fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        let grandparent = parent[parent[x]];
        parent[x] = grandparent;
        x = grandparent;
    }
    x
}

impl AgglomerativeClustering {
    pub fn new(distance_threshold: f32, target_clusters: Option<usize>) -> Self {
        Self {
            distance_threshold,
            target_clusters,
            ..Self::default()
        }
    }

    /// Same, with an explicit linkage rule.
    #[must_use]
    pub fn with_method(
        distance_threshold: f32,
        target_clusters: Option<usize>,
        method: Method,
    ) -> Self {
        Self {
            distance_threshold,
            target_clusters,
            method,
        }
    }

    /// Cluster a list of embedding vectors into speaker indices (0, 1, ...).
    #[must_use]
    pub fn cluster(&self, embeddings: &[Vec<f32>]) -> Vec<usize> {
        let n = embeddings.len();
        if n == 0 {
            return Vec::new();
        }
        if n == 1 {
            return vec![0];
        }

        // Condensed pairwise dissimilarity matrix: the upper triangle, rows laid
        // out contiguously, which is exactly the order kodama expects. Every
        // value is finite because `cosine_distance` returns 1.0 for degenerate
        // (zero-norm or mismatched) inputs rather than NaN.
        //
        // ponytail: O(n^2) memory, n(n-1)/2 f32s. The caller's 800ms/400ms
        // windowing puts a 1-hour call around 60 MB. If multi-hour calls need to
        // fit, merge adjacent windows into speaker turns before clustering
        // instead of growing this matrix.
        let mut condensed = Vec::with_capacity(n * (n - 1) / 2);
        for a in 0..n {
            for b in (a + 1)..n {
                condensed.push(AcousticFeatureExtractor::cosine_distance(
                    &embeddings[a],
                    &embeddings[b],
                ));
            }
        }

        // nn-chain, O(n^2) overall. This used to rescan every cluster pair and
        // every member pair on each merge, which is O(n^3) at best; a one-hour
        // call meant ~9000 windows and it never finished.
        //
        // Average linkage rather than complete, which is the usual choice for
        // diarization and measurably the better one here: complete linkage needs
        // *every* pair in a cluster below the threshold, so a single outlier
        // window blocks a merge. Measured on real recordings, complete linkage
        // reported 5-12 clusters for audio known to hold two speakers where
        // average linkage reported 3-6 (`tests/speaker_count_probe.rs`).
        let dendrogram = linkage(&mut condensed, n, self.method);
        let steps = dendrogram.steps();

        // Average linkage is monotonic (no inversions), so steps arrive in
        // non-decreasing dissimilarity order. Cutting the dendrogram at the
        // threshold is therefore identical to stopping at the first merge that
        // exceeds it.
        let merges = match self.target_clusters {
            Some(target) => n.saturating_sub(target),
            None => steps
                .iter()
                .take_while(|s| s.dissimilarity <= self.distance_threshold)
                .count(),
        };

        // Union-find over dendrogram labels: observations are `0..n`, and step
        // `k` creates the label `n + k`.
        let mut parent: Vec<usize> = (0..(2 * n - 1)).collect();
        for (k, step) in steps.iter().take(merges).enumerate() {
            let merged_label = n + k;
            let a = find(&mut parent, step.cluster1);
            let b = find(&mut parent, step.cluster2);
            parent[a] = merged_label;
            parent[b] = merged_label;
        }

        // Number speakers by first appearance. Observations arrive in time
        // order, so this keeps "Speaker 1" as whoever talks first — and matches
        // the previous implementation, whose cluster order was the ascending
        // minimum observation index of each cluster.
        let mut id_of_root: HashMap<usize, usize> = HashMap::new();
        let mut assignments = Vec::with_capacity(n);
        for i in 0..n {
            let root = find(&mut parent, i);
            let next_id = id_of_root.len();
            assignments.push(*id_of_root.entry(root).or_insert(next_id));
        }

        assignments
    }

    /// Absorb clusters below `min_size` into the nearest surviving centroid.
    ///
    /// `pyannote/speaker-diarization-3.1` ships `min_cluster_size: 12` alongside
    /// the *same* embedding model this project uses, and measurement here agrees
    /// emphatically: at threshold 0.65 over 14 two-party recordings, the count
    /// came out right 1/14 with no minimum, 4/14 at a minimum of 6, and 8/14 at
    /// 12.
    ///
    /// It is also the fix for the visible symptom that started this: a phantom
    /// second speaker holding one word over 310 ms. A handful of windows is not a
    /// participant, it is the acoustic edge of somebody else's turn.
    #[must_use]
    pub fn absorb_small_clusters(
        labels: &[usize],
        embeddings: &[Vec<f32>],
        min_size: usize,
    ) -> Vec<usize> {
        if labels.len() != embeddings.len() || labels.is_empty() {
            return labels.to_vec();
        }

        let mut counts: HashMap<usize, usize> = HashMap::new();
        for &l in labels {
            *counts.entry(l).or_insert(0) += 1;
        }
        let survivors: Vec<usize> = counts
            .iter()
            .filter(|&(_, &c)| c >= min_size)
            .map(|(&l, _)| l)
            .collect();

        // Nothing to absorb, or nothing large enough to absorb into -- in the
        // latter case every cluster is small, which means the recording is short
        // rather than that the speakers are spurious.
        if survivors.is_empty() || survivors.len() == counts.len() {
            return renumber_by_first_appearance(labels);
        }

        let centroids: Vec<(usize, Vec<f32>)> = survivors
            .iter()
            .filter_map(|&s| {
                let members: Vec<&Vec<f32>> = labels
                    .iter()
                    .zip(embeddings)
                    .filter(|(l, _)| **l == s)
                    .map(|(_, e)| e)
                    .collect();
                let dim = members.first()?.len();
                let mut sum = vec![0.0f32; dim];
                for m in &members {
                    for (acc, v) in sum.iter_mut().zip(m.iter()) {
                        *acc += v;
                    }
                }
                let n = members.len() as f32;
                Some((s, sum.into_iter().map(|v| v / n).collect()))
            })
            .collect();

        let reassigned: Vec<usize> = labels
            .iter()
            .zip(embeddings)
            .map(|(&l, embedding)| {
                if survivors.contains(&l) {
                    return l;
                }
                centroids
                    .iter()
                    .min_by(|a, b| {
                        AcousticFeatureExtractor::cosine_distance(embedding, &a.1)
                            .partial_cmp(&AcousticFeatureExtractor::cosine_distance(
                                embedding, &b.1,
                            ))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map_or(l, |(s, _)| *s)
            })
            .collect();
        renumber_by_first_appearance(&reassigned)
    }
}

/// Renumber labels `0..k` by first appearance, so "Speaker 1" is whoever spoke
/// first. Observations arrive in time order.
fn renumber_by_first_appearance(labels: &[usize]) -> Vec<usize> {
    let mut mapping: HashMap<usize, usize> = HashMap::new();
    labels
        .iter()
        .map(|&label| {
            let next = mapping.len();
            *mapping.entry(label).or_insert(next)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The original O(n^3)-plus implementation, kept only to pin the rewrite's
    /// behaviour. Recomputes complete linkage from scratch on every merge.
    fn cluster_reference(
        embeddings: &[Vec<f32>],
        distance_threshold: f32,
        target_clusters: Option<usize>,
    ) -> Vec<usize> {
        let n = embeddings.len();
        if n == 0 {
            return Vec::new();
        }
        if n == 1 {
            return vec![0];
        }

        let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();

        while clusters.len() > 1 {
            if let Some(target) = target_clusters {
                if clusters.len() <= target {
                    break;
                }
            }

            let mut min_dist = f32::INFINITY;
            let mut best_i = 0;
            let mut best_j = 1;

            for i in 0..clusters.len() {
                for j in (i + 1)..clusters.len() {
                    let mut max_pairwise_dist = -1.0f32;
                    for &idx_a in &clusters[i] {
                        for &idx_b in &clusters[j] {
                            let dist = AcousticFeatureExtractor::cosine_distance(
                                &embeddings[idx_a],
                                &embeddings[idx_b],
                            );
                            if dist > max_pairwise_dist {
                                max_pairwise_dist = dist;
                            }
                        }
                    }

                    if max_pairwise_dist < min_dist {
                        min_dist = max_pairwise_dist;
                        best_i = i;
                        best_j = j;
                    }
                }
            }

            if target_clusters.is_none() && min_dist > distance_threshold {
                break;
            }

            let elements_to_merge = clusters.remove(best_j);
            clusters[best_i].extend(elements_to_merge);
        }

        let mut assignments = vec![0usize; n];
        for (cluster_idx, cluster) in clusters.into_iter().enumerate() {
            for item_idx in cluster {
                assignments[item_idx] = cluster_idx;
            }
        }
        assignments
    }

    /// Deterministic embeddings clustered around `speakers` centroids.
    fn synth_embeddings(count: usize, speakers: usize, dims: usize) -> Vec<Vec<f32>> {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed >> 11) as f32 / (1u64 << 53) as f32) - 0.5
        };

        (0..count)
            .map(|i| {
                let speaker = i % speakers;
                (0..dims)
                    .map(|d| {
                        let centroid = if d % speakers == speaker { 1.0 } else { 0.0 };
                        centroid + next() * 0.2
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn test_ahc_clustering() {
        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![0.95, 0.05, 0.0];
        let v3 = vec![0.0, 1.0, 0.0];
        let v4 = vec![0.0, 0.98, 0.02];

        let ahc = AgglomerativeClustering::default();
        let assignments = ahc.cluster(&[v1, v2, v3, v4]);

        assert_eq!(
            assignments[0], assignments[1],
            "v1 and v2 should cluster together"
        );
        assert_eq!(
            assignments[2], assignments[3],
            "v3 and v4 should cluster together"
        );
        assert_ne!(
            assignments[0], assignments[2],
            "v1 and v3 should belong to different clusters"
        );
    }

    /// The kodama rewrite must still agree with the original O(n^3) loop.
    ///
    /// Pinned against `Method::Complete` because that is the linkage the
    /// reference implements. Production uses average linkage -- measurably better
    /// at recovering a speaker count, see `tests/speaker_count_probe.rs` -- so the
    /// two are compared explicitly rather than through the default.
    #[test]
    fn matches_reference_implementation() {
        for &(count, speakers) in &[(2, 2), (7, 2), (16, 3), (40, 4), (61, 5)] {
            let embeddings = synth_embeddings(count, speakers, 24);

            for &threshold in &[0.05f32, 0.2, 0.45, 0.9, 1.5] {
                let got = AgglomerativeClustering::with_method(threshold, None, Method::Complete)
                    .cluster(&embeddings);
                let want = cluster_reference(&embeddings, threshold, None);
                assert_eq!(
                    got, want,
                    "threshold mode diverged at count={count} speakers={speakers} threshold={threshold}"
                );
            }

            for target in 1..=speakers.min(count) {
                let got =
                    AgglomerativeClustering::with_method(0.45, Some(target), Method::Complete)
                        .cluster(&embeddings);
                let want = cluster_reference(&embeddings, 0.45, Some(target));
                assert_eq!(
                    got, want,
                    "target mode diverged at count={count} target={target}"
                );
                assert_eq!(
                    got.iter().collect::<std::collections::HashSet<_>>().len(),
                    target,
                    "target mode must yield exactly {target} clusters"
                );
            }
        }
    }

    #[test]
    fn handles_degenerate_inputs() {
        let ahc = AgglomerativeClustering::default();
        assert!(ahc.cluster(&[]).is_empty());
        assert_eq!(ahc.cluster(&[vec![1.0, 0.0]]), vec![0]);
        // Zero-norm vectors yield a 1.0 distance rather than NaN, which kodama
        // requires; a threshold above that must still collapse them.
        let zeros = vec![vec![0.0, 0.0], vec![0.0, 0.0]];
        assert_eq!(
            AgglomerativeClustering::new(1.0, None).cluster(&zeros),
            vec![0, 0]
        );
    }

    /// The window count a one-hour call produces. The previous implementation
    /// could not complete this; it must now be near-instant.
    #[test]
    fn scales_to_long_call_window_counts() {
        let embeddings = synth_embeddings(4000, 3, 32);
        let assignments = AgglomerativeClustering::new(0.45, Some(3)).cluster(&embeddings);
        assert_eq!(assignments.len(), 4000);
        assert_eq!(
            assignments
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
    }

    #[test]
    #[ignore = "timing comparison, run with --ignored --nocapture"]
    fn perf_old_vs_new() {
        for n in [200usize, 400, 800, 1600] {
            let e = synth_embeddings(n, 3, 32);
            let t0 = std::time::Instant::now();
            let want = cluster_reference(&e, 0.45, None);
            let old = t0.elapsed();
            let t1 = std::time::Instant::now();
            let got = AgglomerativeClustering::new(0.45, None).cluster(&e);
            let new = t1.elapsed();
            assert_eq!(got, want);
            println!(
                "n={n:>4}  old={old:>12.3?}  new={new:>10.3?}  speedup={:.0}x",
                old.as_secs_f64() / new.as_secs_f64().max(1e-9)
            );
        }
        // 5500 is the measured window count of a real 42.9-minute call: 2200s
        // of speech at the 400ms hop. The old implementation is not run at
        // these sizes because it takes tens of minutes.
        for n in [5500usize, 9000] {
            let e = synth_embeddings(n, 3, 32);
            let t = std::time::Instant::now();
            let _ = AgglomerativeClustering::new(0.45, Some(3)).cluster(&e);
            println!("n={n:>4}  new={:>10.3?}  (old: not viable)", t.elapsed());
        }
    }
}
