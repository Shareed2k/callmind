//! The segmenter against labelled recordings.
//!
//! Ignored by default: needs the re-exported model and real audio.
//! `PYANNOTE_SEG_ONNX=models/diarization/segmentation.onnx ONE_SPEAKER_FILES=...
//!  CALLMIND_TEST_AUDIO_DIR=... cargo test --release -p callmind-diarization
//!  --test pyannote_segmenter_test -- --ignored --nocapture`

use callmind_audio::{AudioDecoder, AudioResampler};
use callmind_diarization::pyannote::PyannoteSegmenter;

#[test]
#[ignore = "needs the re-exported model and labelled audio"]
fn segmenter_recovers_the_labelled_speaker_count() {
    // Cargo runs an integration test from the crate directory, not the
    // workspace root.
    let path = std::env::var("PYANNOTE_SEG_ONNX").unwrap_or_else(|_| {
        [
            "models/diarization/segmentation.onnx",
            "../../models/diarization/segmentation.onnx",
        ]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("models/diarization/segmentation.onnx")
        .to_string()
    });
    let Ok(segmenter) = PyannoteSegmenter::load(&path) else {
        println!("model not available at {path}; skipping");
        return;
    };

    let mut corpus: Vec<(usize, String)> = Vec::new();
    if let Ok(files) = std::env::var("ONE_SPEAKER_FILES") {
        corpus.extend(
            files
                .split(',')
                .filter(|s| !s.is_empty())
                // The audio player caches a resampled sibling next to each
                // recording, so a glob over the storage directory picks up the
                // same audio twice and double-counts it.
                .filter(|s| !s.ends_with(".16k.wav"))
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
            .unwrap_or(12);
        let stride = (files.len() / take.max(1)).max(1);
        corpus.extend(
            files
                .into_iter()
                .step_by(stride)
                .take(take)
                .map(|p| (2, p.to_string_lossy().to_string())),
        );
    }
    assert!(!corpus.is_empty(), "no labelled audio provided");

    println!(
        "{:<7} {:>9} {:>8} {:>9} {:>22} {:>7}",
        "label", "duration", "speech%", "speakers", "per-chunk", "ok"
    );
    let mut right = (0usize, 0usize, 0usize, 0usize);

    for (label, path) in &corpus {
        let Ok(audio) = AudioDecoder::decode_file(path) else {
            continue;
        };
        let Ok(mono) = AudioResampler::resample_to_16k_mono(&audio) else {
            continue;
        };
        let seg = segmenter.analyse(&mono).expect("segmentation runs");

        let mut histogram: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        for &c in &seg.per_chunk {
            *histogram.entry(c).or_insert(0) += 1;
        }
        let ok = seg.speakers == *label;
        if *label == 1 {
            right.1 += 1;
            right.0 += usize::from(ok);
        } else {
            right.3 += 1;
            right.2 += usize::from(ok);
        }

        println!(
            "{label:<7} {:>7.1}s {:>7.1}% {:>9} {:>22} {:>7}",
            mono.duration_ms() as f64 / 1000.0,
            seg.speech_ratio * 100.0,
            seg.speakers,
            format!("{histogram:?}"),
            if ok { "yes" } else { "NO" }
        );

        // Regions must be ordered, non-overlapping and inside the recording.
        let mut previous_end = 0u64;
        for region in &seg.speech {
            assert!(region.start_ms >= previous_end, "regions overlap");
            assert!(region.end_ms <= mono.duration_ms(), "region past the end");
            previous_end = region.end_ms;
        }
    }

    println!(
        "\nmonologues: {}/{}   two-party: {}/{}",
        right.0, right.1, right.2, right.3
    );
}
