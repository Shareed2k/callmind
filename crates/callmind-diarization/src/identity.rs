//! Recognising a speaker across calls.
//!
//! A profile is simply an embedding that has been given a name -- there is no
//! separate table of profiles, and no centroid to keep up to date. Naming the
//! same voice in several calls just leaves several exemplars, and the nearest one
//! decides, which handles a voice that sounds different on a different handset
//! better than one averaged vector would.

use crate::features::AcousticFeatureExtractor;

/// A voice that has been given a name.
#[derive(Debug, Clone)]
pub struct KnownSpeaker {
    pub name: String,
    pub embedding: Vec<f32>,
}

/// A match, with the distance so a caller can log or reject it.
#[derive(Debug, Clone)]
pub struct SpeakerMatch {
    pub name: String,
    pub distance: f32,
}

/// Distance at which two embeddings are taken to be the same voice.
///
/// Chosen for precision over recall, from measurements on real recordings: the
/// same person in two calls scored **0.081**, while a call between different
/// people scored **0.605** against that exemplar -- and at the 0.70 pyannote
/// uses for clustering *within* one call, that stranger was given somebody's
/// name.
///
/// The wider distributions say the same thing: same-speaker window pairs run to
/// a p75 of 0.570 and different people start at a p05 of 0.571, so the two
/// almost touch and no value gives both high precision and high recall. Naming
/// somebody's family wrongly is the worse failure, so this sits low: a voice
/// that is not recognised stays a number, which is merely unhelpful.
pub const SAME_SPEAKER_DISTANCE: f32 = 0.45;

/// One vector per speaker: the mean of the windows assigned to them.
///
/// This is what gets stored as the call's voice prints. Returns nothing when the
/// inputs do not line up rather than averaging whatever happens to pair -- a
/// centroid built from the wrong windows would be worse than none, because it
/// would be silently matched against in every later call.
#[must_use]
pub fn speaker_centroids(labels: &[usize], embeddings: &[Vec<f32>]) -> Vec<(usize, Vec<f32>)> {
    if labels.len() != embeddings.len() {
        return Vec::new();
    }

    let mut sums: std::collections::BTreeMap<usize, (Vec<f32>, usize)> =
        std::collections::BTreeMap::new();
    for (&label, embedding) in labels.iter().zip(embeddings) {
        let entry = sums
            .entry(label)
            .or_insert_with(|| (vec![0.0; embedding.len()], 0));
        if entry.0.len() == embedding.len() {
            for (acc, v) in entry.0.iter_mut().zip(embedding) {
                *acc += v;
            }
            entry.1 += 1;
        }
    }

    sums.into_iter()
        .filter(|(_, (_, count))| *count > 0)
        .map(|(label, (sum, count))| {
            let n = count as f32;
            (label, sum.into_iter().map(|v| v / n).collect())
        })
        .collect()
}

/// Find the named voice closest to `probe`, if any is close enough.
///
/// Returns `None` rather than the nearest name when nothing is within
/// `max_distance`: labelling a stranger with somebody's family member is a worse
/// failure than leaving a speaker numbered.
#[must_use]
pub fn identify(probe: &[f32], known: &[KnownSpeaker], max_distance: f32) -> Option<SpeakerMatch> {
    known
        .iter()
        // A mismatched dimension scores 1.0, so it falls out with everything
        // else too far away rather than needing its own branch.
        .map(|k| {
            (
                k,
                AcousticFeatureExtractor::cosine_distance(probe, &k.embedding),
            )
        })
        .filter(|(_, distance)| *distance <= max_distance)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(k, distance)| SpeakerMatch {
            name: k.name.clone(),
            distance,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One vector per speaker is what gets stored, so the averaging has to be
    /// right: a wrong centroid means a voice is never recognised again.
    #[test]
    fn a_centroid_is_the_mean_of_that_speakers_windows() {
        let embeddings = vec![vec![1.0, 0.0], vec![0.0, 2.0], vec![3.0, 3.0]];
        let labels = [0usize, 0, 1];

        let centroids = speaker_centroids(&labels, &embeddings);

        assert_eq!(centroids.len(), 2);
        assert_eq!(centroids[0], (0, vec![0.5, 1.0]), "mean of the first two");
        assert_eq!(centroids[1], (1, vec![3.0, 3.0]), "a lone window is itself");
    }

    #[test]
    fn centroids_ignore_mismatched_and_missing_input() {
        assert!(speaker_centroids(&[], &[]).is_empty());
        // More labels than embeddings is a bug elsewhere; produce nothing rather
        // than a centroid built from whatever happened to line up.
        assert!(speaker_centroids(&[0, 1], &[vec![1.0, 0.0]]).is_empty());
    }

    fn exemplar(name: &str, v: [f32; 3]) -> KnownSpeaker {
        KnownSpeaker {
            name: name.to_string(),
            embedding: v.to_vec(),
        }
    }

    /// The point of the feature: a voice already named in one call is recognised
    /// in the next one.
    #[test]
    fn a_named_voice_is_recognised_again() {
        let known = vec![
            exemplar("Dad", [1.0, 0.0, 0.0]),
            exemplar("Mum", [0.0, 1.0, 0.0]),
        ];
        // Close to Dad, not identical -- a different recording of the same voice.
        let probe = vec![0.96, 0.05, 0.02];

        let hit = identify(&probe, &known, SAME_SPEAKER_DISTANCE).expect("a match");
        assert_eq!(hit.name, "Dad");
    }

    /// A stranger must stay a stranger rather than being forced onto the nearest
    /// name -- mislabelling somebody's family is worse than leaving a number.
    #[test]
    fn an_unknown_voice_is_not_forced_onto_the_nearest_name() {
        let known = vec![exemplar("Dad", [1.0, 0.0, 0.0])];
        let probe = vec![0.0, 0.0, 1.0];
        assert!(identify(&probe, &known, SAME_SPEAKER_DISTANCE).is_none());
    }

    #[test]
    fn the_nearest_of_several_exemplars_wins() {
        // The same person may be named in several calls; each is an exemplar and
        // the closest one decides.
        let known = vec![
            exemplar("Dad", [0.8, 0.2, 0.0]),
            exemplar("Dad", [0.99, 0.01, 0.0]),
            exemplar("Mum", [0.0, 1.0, 0.0]),
        ];
        let hit = identify(&[1.0, 0.0, 0.0], &known, SAME_SPEAKER_DISTANCE).expect("a match");
        assert_eq!(hit.name, "Dad");
        assert!(hit.distance < 0.05, "distance was {}", hit.distance);
    }

    /// Measured on real recordings: the same voice in two calls scored 0.081,
    /// while a call between different people scored 0.605 against that exemplar
    /// and was wrongly labelled. The threshold has to sit between them.
    #[test]
    fn a_voice_at_the_observed_stranger_distance_is_rejected() {
        // Unit vectors with a dot product of 0.4, so cosine distance is 0.60 --
        // the distance at which a stranger was accepted.
        let known = vec![exemplar("Роман", [1.0, 0.0, 0.0])];
        let stranger = vec![0.4, 0.916_515_1, 0.0];

        let distance = crate::features::AcousticFeatureExtractor::cosine_distance(
            &stranger,
            &known[0].embedding,
        );
        assert!(
            (0.59..=0.61).contains(&distance),
            "the fixture should sit at ~0.60, was {distance}"
        );

        assert!(
            identify(&stranger, &known, SAME_SPEAKER_DISTANCE).is_none(),
            "a stranger at 0.60 must not be given somebody's name"
        );
    }

    /// And the true match measured on the same recordings must still be found.
    #[test]
    fn a_voice_at_the_observed_same_speaker_distance_is_accepted() {
        let known = vec![exemplar("Роман", [1.0, 0.0, 0.0])];
        // Dot product 0.919 -> distance 0.081, the figure observed between two
        // recordings of one person.
        let same = vec![0.919, 0.394_2, 0.0];
        let hit = identify(&same, &known, SAME_SPEAKER_DISTANCE).expect("the same voice");
        assert_eq!(hit.name, "Роман");
    }

    #[test]
    fn degenerate_input_matches_nothing() {
        assert!(identify(&[], &[exemplar("Dad", [1.0, 0.0, 0.0])], 0.7).is_none());
        assert!(identify(&[1.0, 0.0, 0.0], &[], 0.7).is_none());
        // A mismatched dimension is a bug elsewhere, not a match.
        let odd = KnownSpeaker {
            name: "Dad".into(),
            embedding: vec![1.0, 0.0],
        };
        assert!(identify(&[1.0, 0.0, 0.0], &[odd], 0.7).is_none());
    }
}
