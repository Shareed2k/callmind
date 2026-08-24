//! Writing speaker names into a stored transcript.
//!
//! One implementation for both callers: the rename endpoint, where a person types
//! a name, and the pipeline, which applies a name recognised from a previous
//! call. Two copies of this would drift on the shape of the stored JSON, which is
//! the only record of what was said.

use std::collections::HashMap;

/// Set `speaker_label` on every segment belonging to a named speaker.
///
/// Returns how many segments changed, so a caller can skip the write when there
/// is nothing to save. Leaves an unexpected shape alone rather than rewriting
/// half of it.
pub fn apply_speaker_labels<S: std::hash::BuildHasher>(
    transcript: &mut serde_json::Value,
    names: &HashMap<u16, String, S>,
) -> usize {
    if names.is_empty() {
        return 0;
    }
    let Some(segments) = transcript
        .get_mut("segments")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return 0;
    };

    let mut changed = 0;
    for segment in segments {
        let Some(speaker) = segment
            .get("speaker_id")
            .and_then(serde_json::Value::as_u64)
        else {
            continue;
        };
        if let Some(name) = names.get(&(speaker as u16)) {
            segment["speaker_label"] = serde_json::Value::String(name.clone());
            changed += 1;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(speaker_ids: &[u64]) -> serde_json::Value {
        serde_json::json!({
            "segments": speaker_ids
                .iter()
                .map(|id| serde_json::json!({ "speaker_id": id, "normalized_text": "текст" }))
                .collect::<Vec<_>>()
        })
    }

    #[test]
    fn a_named_speaker_gets_the_label_on_every_segment_they_speak() {
        let mut json = transcript(&[0, 1, 0]);
        let names: HashMap<u16, String> = [(0u16, "Папа".to_string())].into_iter().collect();

        let changed = apply_speaker_labels(&mut json, &names);

        assert_eq!(changed, 2, "both of speaker 0's segments");
        let segs = json["segments"].as_array().unwrap();
        assert_eq!(segs[0]["speaker_label"], "Папа");
        assert_eq!(segs[2]["speaker_label"], "Папа");
        assert!(
            segs[1].get("speaker_label").is_none(),
            "speaker 1 was not named, so nothing is claimed about them"
        );
    }

    #[test]
    fn naming_nobody_changes_nothing() {
        let mut json = transcript(&[0, 1]);
        let before = json.clone();
        assert_eq!(apply_speaker_labels(&mut json, &HashMap::new()), 0);
        assert_eq!(json, before);
    }

    /// A transcript that is not the expected shape must be left alone rather than
    /// half-rewritten -- it is the only copy of what was said.
    #[test]
    fn a_transcript_without_segments_is_untouched() {
        let mut json = serde_json::json!({ "unexpected": true });
        let before = json.clone();
        let names: HashMap<u16, String> = [(0u16, "Папа".to_string())].into_iter().collect();
        assert_eq!(apply_speaker_labels(&mut json, &names), 0);
        assert_eq!(json, before);
    }

    #[test]
    fn relabelling_replaces_the_previous_name() {
        let mut json = transcript(&[0]);
        let first: HashMap<u16, String> = [(0u16, "Папа".to_string())].into_iter().collect();
        apply_speaker_labels(&mut json, &first);
        let second: HashMap<u16, String> = [(0u16, "Отец".to_string())].into_iter().collect();
        assert_eq!(apply_speaker_labels(&mut json, &second), 1);
        assert_eq!(json["segments"][0]["speaker_label"], "Отец");
    }
}
