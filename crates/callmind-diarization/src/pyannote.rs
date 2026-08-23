//! Speaker segmentation with pyannote, which is what makes counting possible.
//!
//! Every other approach in this crate clusters embeddings of fixed windows and
//! then tries to infer how many speakers there are from the distances. That was
//! measured repeatedly and does not work: with the WeSpeaker embeddings this
//! project uses, one voice spread across windows overlaps heavily with two
//! different voices, so no threshold separates the cases.
//!
//! This model answers a different, easier question directly. It classifies each
//! frame into a **powerset** of active speakers -- seven classes over up to three
//! speakers, including the two-at-once combinations -- so a single 10-second
//! chunk already reports whether one person or two are talking, with no
//! clustering and no distance threshold anywhere.
//!
//! Measured against labelled recordings (four monologues confirmed by their
//! owner, thirty two-party phone calls), taking the **median** per-chunk count:
//!
//! | statistic | monologues | two-party |
//! | :--- | ---: | ---: |
//! | maximum per chunk | 4/4 | 20/30 |
//! | **median per chunk** | **4/4** | **23/24** |
//!
//! The two come from different samples: the maximum was scored on thirty calls
//! before the median was adopted, the median on twenty-four spanning 1 s to
//! 13 min. The single miss is a 9.9-second call reported as three speakers.
//!
//! The maximum is the wrong statistic even though it looks natural: it is the
//! maximum of a noisy quantity, so it grows with the number of chunks and a long
//! call reliably reports one speaker too many.
//!
//! The model file is a derived artifact: the published export cannot be loaded by
//! `tract` and has to be re-exported with a fixed input shape first. See
//! `scripts/export_pyannote_segmentation.py`, which also verifies that the
//! transformation leaves the output bit-identical.

use crate::errors::DiarizationError;
use callmind_audio::AudioBuffer;
use callmind_vad::SpeechRegion;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tract_onnx::prelude::*;

/// Samples per inference, fixed by the re-exported graph: 10 s at 16 kHz.
const CHUNK_SAMPLES: usize = 160_000;
/// Sample rate the model is built for.
pub const SAMPLE_RATE: u32 = 16_000;

/// pyannote 3.0's powerset: which speakers each of the seven classes means.
///
/// Class 0 is silence; the last three are the two-speakers-at-once combinations,
/// which is how the model represents overlapping speech.
const POWERSET: [&[usize]; 7] = [&[], &[0], &[1], &[2], &[0, 1], &[0, 2], &[1, 2]];

/// What one pass over a recording found.
#[derive(Debug, Clone)]
pub struct Segmentation {
    /// Speech regions, from the frames the model did not call silence.
    pub speech: Vec<SpeechRegion>,
    /// How many speakers the recording holds.
    pub speakers: usize,
    /// Distinct speakers seen in each chunk, in order. Kept so a caller can log
    /// why the count came out as it did -- the median of these is `speakers`.
    pub per_chunk: Vec<usize>,
    /// Fraction of frames that were speech.
    pub speech_ratio: f32,
}

pub struct PyannoteSegmenter {
    plan: Arc<TypedRunnableModel>,
    model_path: PathBuf,
    /// Gaps shorter than this do not split a speech region.
    pub min_silence_duration_ms: u64,
    /// Regions shorter than this are dropped.
    pub min_speech_duration_ms: u64,
}

impl std::fmt::Debug for PyannoteSegmenter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyannoteSegmenter")
            .field("model_path", &self.model_path)
            .finish_non_exhaustive()
    }
}

impl PyannoteSegmenter {
    /// Load the re-exported segmentation model.
    ///
    /// Fails with a pointed message on the stock export, because that is the
    /// mistake somebody will make first.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, DiarizationError> {
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            return Err(DiarizationError::Inference(format!(
                "pyannote segmentation model not found at {}",
                path.display()
            )));
        }

        let plan = tract_onnx::onnx()
            .model_for_path(&path)
            .and_then(tract_onnx::prelude::Graph::into_typed)
            .and_then(tract_onnx::prelude::Graph::into_decluttered)
            .and_then(tract_onnx::prelude::Graph::into_optimized)
            .and_then(tract_onnx::prelude::Graph::into_runnable)
            .map_err(|e| {
                DiarizationError::Inference(format!(
                    "pyannote segmentation model failed to load: {e}. The published export \
                     cannot be used directly -- it contains a shape-conditional `If` node that \
                     tract cannot translate. Re-export it with \
                     scripts/export_pyannote_segmentation.py first."
                ))
            })?;

        Ok(Self {
            plan,
            model_path: path,
            min_silence_duration_ms: 300,
            min_speech_duration_ms: 250,
        })
    }

    #[must_use]
    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    /// Segment a recording: speech regions and how many speakers it holds.
    ///
    /// Synchronous and CPU-bound; callers run it inside `spawn_blocking`.
    pub fn analyse(&self, audio: &AudioBuffer) -> Result<Segmentation, DiarizationError> {
        let mono = audio.to_mono();
        if mono.samples.is_empty() {
            return Err(DiarizationError::EmptyAudio);
        }
        if mono.sample_rate != SAMPLE_RATE {
            return Err(DiarizationError::Inference(format!(
                "pyannote segmentation needs {SAMPLE_RATE} Hz, got {}. Resample first.",
                mono.sample_rate
            )));
        }

        let mut per_chunk = Vec::new();
        let mut speech_frames = 0usize;
        let mut total_frames = 0usize;
        // Frame-level speech decisions across the whole recording, so regions can
        // be built once at the end rather than stitched per chunk.
        let mut voiced: Vec<bool> = Vec::new();
        let mut frame_ms = 0.0f64;

        let mut offset = 0usize;
        while offset < mono.samples.len() {
            // The tail is zero-padded rather than dropped: the graph needs a fixed
            // window and the last chunk still holds speech.
            let mut window = vec![0.0f32; CHUNK_SAMPLES];
            let take = CHUNK_SAMPLES.min(mono.samples.len() - offset);
            window[..take].copy_from_slice(&mono.samples[offset..offset + take]);

            let input: Tensor =
                tract_ndarray::Array3::from_shape_vec((1, 1, CHUNK_SAMPLES), window)
                    .map_err(|e| {
                        DiarizationError::Inference(format!("segmentation input shape: {e}"))
                    })?
                    .into();
            let output = self.plan.run(tvec!(input.into())).map_err(|e| {
                DiarizationError::Inference(format!("segmentation inference failed: {e}"))
            })?;
            let view = output[0].to_plain_array_view::<f32>().map_err(|e| {
                DiarizationError::Inference(format!("segmentation output read: {e}"))
            })?;
            let flat: Vec<f32> = view.iter().copied().collect();
            let frames = flat.len() / POWERSET.len();
            if frames == 0 {
                break;
            }
            // Derived from the actual output rather than assumed, so a different
            // export cannot silently shift every timestamp.
            frame_ms = 10_000.0 / frames as f64;

            let mut seen = [false; 3];
            // How much of this chunk is real audio rather than the zero padding.
            let real_frames = (take as f64 / CHUNK_SAMPLES as f64 * frames as f64).ceil() as usize;
            for (i, frame) in flat.chunks(POWERSET.len()).enumerate() {
                let class = frame
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .map_or(0, |(i, _)| i);
                if i >= real_frames {
                    continue;
                }
                total_frames += 1;
                let speaking = class != 0;
                voiced.push(speaking);
                if speaking {
                    speech_frames += 1;
                }
                for &s in POWERSET[class] {
                    seen[s] = true;
                }
            }

            per_chunk.push(seen.iter().filter(|s| **s).count().max(1));
            offset += CHUNK_SAMPLES;
        }

        let speakers = median(&per_chunk);
        Ok(Segmentation {
            speech: self.regions_from_frames(&voiced, frame_ms, mono.duration_ms()),
            speakers,
            per_chunk,
            speech_ratio: if total_frames == 0 {
                0.0
            } else {
                speech_frames as f32 / total_frames as f32
            },
        })
    }

    /// Merge frame decisions into padded speech regions.
    fn regions_from_frames(
        &self,
        voiced: &[bool],
        frame_ms: f64,
        total_ms: u64,
    ) -> Vec<SpeechRegion> {
        if frame_ms <= 0.0 {
            return Vec::new();
        }
        let at = |i: usize| (i as f64 * frame_ms) as u64;

        let mut runs: Vec<(u64, u64)> = Vec::new();
        let mut start: Option<usize> = None;
        for (i, &speaking) in voiced.iter().enumerate() {
            match (speaking, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    runs.push((at(s), at(i)));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            runs.push((at(s), at(voiced.len())));
        }

        // A pause mid-sentence is not a region boundary; splitting there
        // fragments the speaker turns downstream.
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for run in runs {
            match merged.last_mut() {
                Some(last) if run.0.saturating_sub(last.1) < self.min_silence_duration_ms => {
                    last.1 = run.1;
                }
                _ => merged.push(run),
            }
        }

        merged
            .into_iter()
            .filter(|(s, e)| e.saturating_sub(*s) >= self.min_speech_duration_ms)
            .map(|(s, e)| SpeechRegion::new(s, e.min(total_ms), 0.95))
            .collect()
    }
}

/// Median of the per-chunk counts, which is what the measurement selected.
fn median(counts: &[usize]) -> usize {
    if counts.is_empty() {
        return 1;
    }
    let mut sorted = counts.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2].max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_model_is_an_error_not_a_panic() {
        assert!(PyannoteSegmenter::load("/nonexistent/segmentation.onnx").is_err());
    }

    #[test]
    fn median_ignores_the_noisy_tail() {
        // The shape that made the maximum wrong: mostly two, with a stray three.
        assert_eq!(median(&[1, 2, 2, 2, 3]), 2);
        assert_eq!(median(&[2, 2, 2, 2]), 2);
        assert_eq!(median(&[1, 1, 1]), 1);
        assert_eq!(median(&[]), 1);
        // Never zero, even if a chunk somehow reported no speaker at all.
        assert_eq!(median(&[0, 0, 0]), 1);
    }

    /// The powerset table is the contract with the model; a wrong entry would
    /// mis-count speakers with no error anywhere.
    #[test]
    fn powerset_covers_three_speakers_and_their_pairs() {
        assert_eq!(POWERSET.len(), 7, "the graph emits seven classes");
        assert!(POWERSET[0].is_empty(), "class 0 is silence");
        let singles: Vec<_> = POWERSET.iter().filter(|s| s.len() == 1).collect();
        let pairs: Vec<_> = POWERSET.iter().filter(|s| s.len() == 2).collect();
        assert_eq!(singles.len(), 3);
        assert_eq!(pairs.len(), 3);
        let mut speakers: Vec<usize> = POWERSET.iter().flat_map(|s| s.iter().copied()).collect();
        speakers.sort_unstable();
        speakers.dedup();
        assert_eq!(speakers, vec![0, 1, 2]);
    }
}
