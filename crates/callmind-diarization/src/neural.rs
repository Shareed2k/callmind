use async_trait::async_trait;
use callmind_core::SpeakerId;
use callmind_vad::{EnergyVadEngine, VadEngine};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

use crate::ahc::AgglomerativeClustering;
use crate::clustering::ClusteringDiarizer;
use crate::errors::DiarizationError;
use crate::models::{DiarizationRequest, DiarizationResult, SpeakerTurn};
use crate::onnx_extractor::OnnxSpeakerEmbeddingExtractor;
use crate::traits::DiarizationEngine;

/// Neural Speaker Diarization Engine using ONNX embeddings with seamless DSP fallback.
pub struct NeuralDiarizer {
    /// Supplies the speaker count when present. See [`Self::with_segmenter`].
    segmenter: Option<Arc<crate::pyannote::PyannoteSegmenter>>,
    extractor: Option<Arc<OnnxSpeakerEmbeddingExtractor>>,
    fallback: ClusteringDiarizer,
    vad: Arc<dyn VadEngine>,
}

impl NeuralDiarizer {
    /// Creates a `NeuralDiarizer` attempting to load an ONNX model from the specified path,
    /// falling back to acoustic clustering if the file is missing or invalid.
    pub fn new_with_fallback(model_path: Option<PathBuf>, vad: Arc<dyn VadEngine>) -> Self {
        let fallback = ClusteringDiarizer::new(Arc::clone(&vad));
        let extractor = if let Some(path) = model_path {
            if path.exists() {
                match OnnxSpeakerEmbeddingExtractor::load(&path) {
                    Ok(ext) => {
                        info!(
                            "Loaded neural speaker embedding ONNX model from {}",
                            path.display()
                        );
                        Some(Arc::new(ext))
                    }
                    Err(e) => {
                        warn!(
                            "Failed to load ONNX model from {}: {e}. Using acoustic DSP fallback.",
                            path.display()
                        );
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        Self {
            extractor,
            fallback,
            vad,
            segmenter: None,
        }
    }

    /// Attach pyannote segmentation, which supplies the speaker count.
    ///
    /// Optional and feature-detected: with the model present the count comes from
    /// the audio, without it the two-party prior applies. Measured on labelled
    /// recordings -- four confirmed by their owner as one voice, twenty-four
    /// two-party phone calls -- segmentation identified 4/4 and 23/24, against
    /// 0/4 and 24/24 for the prior.
    #[must_use]
    pub fn with_segmenter(mut self, segmenter: Arc<crate::pyannote::PyannoteSegmenter>) -> Self {
        self.segmenter = Some(segmenter);
        self
    }
}

impl Default for NeuralDiarizer {
    fn default() -> Self {
        Self::new_with_fallback(None, Arc::new(EnergyVadEngine::default()))
    }
}

#[async_trait]
impl DiarizationEngine for NeuralDiarizer {
    async fn diarize(
        &self,
        request: DiarizationRequest<'_>,
    ) -> Result<DiarizationResult, DiarizationError> {
        let audio = request.audio;
        if audio.is_empty() {
            return Err(DiarizationError::EmptyAudio);
        }

        // If no ONNX model loaded, route directly to the acoustic fallback diarizer
        let Some(extractor) = &self.extractor else {
            return self.fallback.diarize(request).await;
        };

        let mono = audio.to_mono();

        // With segmentation available, one pass supplies both the speech regions
        // and the speaker count -- it is a neural voice activity detector as well
        // as a diarizer, so this replaces the energy detector rather than running
        // alongside it. CPU-bound, hence `spawn_blocking`.
        let (regions, measured_speakers) = match self.segmenter.clone() {
            Some(segmenter) => {
                let audio_for_seg = mono.clone();
                let outcome =
                    tokio::task::spawn_blocking(move || segmenter.analyse(&audio_for_seg))
                        .await
                        .map_err(|e| {
                            DiarizationError::Inference(format!("segmentation task failed: {e}"))
                        })?;
                match outcome {
                    Ok(seg) => {
                        tracing::debug!(
                            speakers = seg.speakers,
                            chunks = seg.per_chunk.len(),
                            speech_ratio = seg.speech_ratio,
                            "pyannote segmentation"
                        );
                        (seg.speech, Some(seg.speakers))
                    }
                    Err(e) => {
                        // A failure here must not fail the call: fall back to the
                        // energy detector and the two-party prior.
                        warn!("Segmentation failed ({e}); falling back to the energy detector.");
                        (self.vad.detect(&mono).await?, None)
                    }
                }
            }
            None => (self.vad.detect(&mono).await?, None),
        };

        if regions.is_empty() {
            let single_turn =
                SpeakerTurn::new(SpeakerId::new(0), 0, audio.duration_ms(), Some(1.0));
            return Ok(DiarizationResult::new(1, vec![single_turn]));
        }

        let sample_rate = audio.sample_rate as usize;
        // A count supplied by the caller wins outright -- a stereo recording has
        // two channels and that is not a guess. Absent one, the count is
        // *estimated* from the audio rather than assumed: assuming two split
        // every monologue in half, which is the bug that started this.
        let hinted_speakers = request.expected_speakers.map(|k| k.clamp(1, 10));
        let extractor = Arc::clone(extractor);

        // ONNX inference over every window plus the AHC pass are both pure
        // CPU work — thousands of `plan.run()` calls and an O(n^2) clustering.
        // Running them inline blocked a runtime worker thread, and the
        // `try_join!` in the transcriber polls this on the same task as STT, so
        // it also stalled the STT future it was supposed to overlap with.
        let outcome = tokio::task::spawn_blocking(move || -> Option<DiarizationResult> {
            let window_ms: u64 = 800;
            let hop_ms: u64 = 400;
            // Enough audio to be worth embedding: 50ms.
            let min_samples = sample_rate / 20;

            // Plan the windows first, then embed them in parallel. Each ONNX
            // inference is independent and measured at ~49ms, so a one-hour call
            // spent ~270s of a single core running them one at a time.
            let mut sub_segments: Vec<(u64, u64)> = Vec::new();
            for region in &regions {
                let push = |from: u64, to: u64, out: &mut Vec<(u64, u64)>| {
                    let start_sample = (from as usize * sample_rate) / 1000;
                    let end_sample = (to as usize * sample_rate) / 1000;
                    if end_sample > start_sample + min_samples {
                        out.push((from, to));
                    }
                };

                if region.duration_ms() <= window_ms {
                    push(region.start_ms, region.end_ms, &mut sub_segments);
                } else {
                    let mut curr = region.start_ms;
                    while curr + window_ms <= region.end_ms {
                        let s_end = (curr + window_ms).min(region.end_ms);
                        push(curr, s_end, &mut sub_segments);
                        curr += hop_ms;
                    }
                    if curr < region.end_ms {
                        push(curr, region.end_ms, &mut sub_segments);
                    }
                }
            }

            // Deliberately conservative: speech-to-text runs concurrently with
            // this via `try_join!` and whisper.cpp is itself multi-threaded, so
            // saturating the machine here just moves the cost onto the critical
            // path. A quarter of the cores already takes the stage from ~270s to
            // well under a minute on a one-hour call.
            //
            // ponytail: fixed fraction rather than a tuned scheduler. If STT and
            // diarization ever need to share a real budget, make it configurable.
            let threads = std::thread::available_parallelism()
                .map_or(2, std::num::NonZeroUsize::get)
                .saturating_div(4)
                .clamp(2, 4);
            let chunk_size = sub_segments.len().div_ceil(threads).max(1);

            let mut chunk_results: Vec<Option<Vec<Vec<f32>>>> = Vec::new();
            std::thread::scope(|scope| {
                let handles: Vec<_> = sub_segments
                    .chunks(chunk_size)
                    .map(|chunk| {
                        let extractor = &extractor;
                        let samples = &mono.samples;
                        scope.spawn(move || {
                            let mut out = Vec::with_capacity(chunk.len());
                            for &(from, to) in chunk {
                                let start_sample = (from as usize * sample_rate) / 1000;
                                let end_sample =
                                    ((to as usize * sample_rate) / 1000).min(samples.len());
                                match extractor
                                    .extract_embedding(&samples[start_sample..end_sample])
                                {
                                    Ok(emb) => out.push(emb),
                                    // Any failure routes the whole call to the
                                    // acoustic fallback, as before.
                                    Err(_) => return None,
                                }
                            }
                            Some(out)
                        })
                    })
                    .collect();

                for handle in handles {
                    chunk_results.push(handle.join().unwrap_or(None));
                }
            });

            let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(sub_segments.len());
            for chunk in chunk_results {
                embeddings.extend(chunk?);
            }

            if embeddings.is_empty() {
                return None;
            }

            // Without a hint, estimate the count from a *separate* set of
            // 2-second windows. The length matters: the estimator scores 4/4 on
            // monologues and 10/14 on two-party calls at 2000 ms, and 2/4 and
            // 7/14 at the 800 ms used above. Turn assignment keeps the shorter
            // window because that is what sets boundary resolution, and the
            // estimate is capped at 256 windows, so the extra inference is
            // bounded rather than proportional to call length.
            // Order of authority: an explicit hint, then what segmentation
            // measured from the audio, then a two-party prior. Only the last is a
            // guess, and it is now the fallback rather than the rule.
            let target_speakers = hinted_speakers
                .or(measured_speakers)
                .unwrap_or(2)
                .clamp(1, 10);

            // No minimum-cluster filter here. `min_cluster_size` was measured on
            // the 2-second counting windows and belongs to the estimator; applied
            // to these 800 ms labels it is a different density entirely, and on a
            // lopsided call -- one measured at 3.9 s against 69 s of speech -- it
            // absorbed the quieter participant and reported a single speaker.
            let ahc = AgglomerativeClustering::new(
                crate::spectral::SPEAKER_DISTANCE_THRESHOLD,
                Some(target_speakers),
            );
            let labels = ahc.cluster(&embeddings);

            let mut raw_turns: Vec<SpeakerTurn> = Vec::new();
            for ((start_ms, end_ms), &cluster_id) in sub_segments.into_iter().zip(labels.iter()) {
                let speaker_id = SpeakerId::new(cluster_id as u16);
                if let Some(last) = raw_turns.last_mut() {
                    if last.speaker == speaker_id && start_ms <= last.end_ms + 200 {
                        last.end_ms = last.end_ms.max(end_ms);
                        continue;
                    }
                }
                raw_turns.push(SpeakerTurn::new(speaker_id, start_ms, end_ms, Some(0.92)));
            }

            let num_distinct_speakers = raw_turns
                .iter()
                .map(|t| t.speaker.as_u16())
                .max()
                .map_or(1, |m| (m + 1) as usize);

            // The embeddings are already in hand; one centroid per speaker is
            // what makes the same voice recognisable in a later call.
            let centroids = crate::identity::speaker_centroids(&labels, &embeddings)
                .into_iter()
                .map(|(label, centroid)| (SpeakerId::new(label as u16), centroid))
                .collect();

            Some(
                DiarizationResult::new(num_distinct_speakers, raw_turns)
                    .with_speaker_embeddings(centroids),
            )
        })
        .await
        .map_err(|e| DiarizationError::Inference(format!("diarization task failed: {e}")))?;

        // Any embedding failure or an empty result routes to the acoustic
        // fallback, which has to happen out here because it is async.
        let Some(result) = outcome else {
            return self.fallback.diarize(request).await;
        };

        Ok(result)
    }
}
