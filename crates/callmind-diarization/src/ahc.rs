use crate::features::AcousticFeatureExtractor;

/// Agglomerative Hierarchical Clustering (AHC) with complete linkage and distance thresholding.
pub struct AgglomerativeClustering {
    pub distance_threshold: f32,
    pub target_clusters: Option<usize>,
}

impl Default for AgglomerativeClustering {
    fn default() -> Self {
        Self {
            distance_threshold: 0.45,
            target_clusters: None,
        }
    }
}

impl AgglomerativeClustering {
    pub fn new(distance_threshold: f32, target_clusters: Option<usize>) -> Self {
        Self {
            distance_threshold,
            target_clusters,
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

        // Initialize each embedding in its own cluster
        let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();

        // Iteratively merge closest clusters
        while clusters.len() > 1 {
            // Check if target cluster count has been reached
            if let Some(target) = self.target_clusters {
                if clusters.len() <= target {
                    break;
                }
            }

            let mut min_dist = f32::INFINITY;
            let mut best_i = 0;
            let mut best_j = 1;

            for i in 0..clusters.len() {
                for j in (i + 1)..clusters.len() {
                    // Complete linkage: max distance between all pairwise elements
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

            // If distance threshold is exceeded and no target cluster count is forced, stop merging
            if self.target_clusters.is_none() && min_dist > self.distance_threshold {
                break;
            }

            // Merge cluster j into cluster i and remove cluster j
            let elements_to_merge = clusters.remove(best_j);
            clusters[best_i].extend(elements_to_merge);
        }

        // Generate final assignments vector
        let mut assignments = vec![0usize; n];
        for (cluster_idx, cluster) in clusters.into_iter().enumerate() {
            for item_idx in cluster {
                assignments[item_idx] = cluster_idx;
            }
        }

        assignments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
