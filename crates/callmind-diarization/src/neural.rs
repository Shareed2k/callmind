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
        }
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
        let regions = self.vad.detect(&mono).await?;

        if regions.is_empty() {
            let single_turn =
                SpeakerTurn::new(SpeakerId::new(0), 0, audio.duration_ms(), Some(1.0));
            return Ok(DiarizationResult::new(1, vec![single_turn]));
        }

        let sample_rate = audio.sample_rate as usize;
        let window_ms: u64 = 800;
        let hop_ms: u64 = 400;

        let mut sub_segments: Vec<(u64, u64)> = Vec::new();
        let mut embeddings = Vec::new();

        for region in &regions {
            let dur = region.duration_ms();
            if dur <= window_ms {
                let start_sample = (region.start_ms as usize * sample_rate) / 1000;
                let end_sample = (region.end_ms as usize * sample_rate) / 1000;
                if end_sample > start_sample + (sample_rate / 20) {
                    let slice = &mono.samples[start_sample..end_sample.min(mono.samples.len())];
                    match extractor.extract_embedding(slice) {
                        Ok(emb) => {
                            embeddings.push(emb);
                            sub_segments.push((region.start_ms, region.end_ms));
                        }
                        Err(_) => return self.fallback.diarize(request).await,
                    }
                }
            } else {
                let mut curr = region.start_ms;
                while curr + window_ms <= region.end_ms {
                    let s_end = (curr + window_ms).min(region.end_ms);
                    let start_sample = (curr as usize * sample_rate) / 1000;
                    let end_sample = (s_end as usize * sample_rate) / 1000;
                    if end_sample > start_sample + (sample_rate / 20) {
                        let slice = &mono.samples[start_sample..end_sample.min(mono.samples.len())];
                        match extractor.extract_embedding(slice) {
                            Ok(emb) => {
                                embeddings.push(emb);
                                sub_segments.push((curr, s_end));
                            }
                            Err(_) => return self.fallback.diarize(request).await,
                        }
                    }
                    curr += hop_ms;
                }
                if curr < region.end_ms {
                    let start_sample = (curr as usize * sample_rate) / 1000;
                    let end_sample = (region.end_ms as usize * sample_rate) / 1000;
                    if end_sample > start_sample + (sample_rate / 20) {
                        let slice = &mono.samples[start_sample..end_sample.min(mono.samples.len())];
                        match extractor.extract_embedding(slice) {
                            Ok(emb) => {
                                embeddings.push(emb);
                                sub_segments.push((curr, region.end_ms));
                            }
                            Err(_) => return self.fallback.diarize(request).await,
                        }
                    }
                }
            }
        }

        if embeddings.is_empty() {
            return self.fallback.diarize(request).await;
        }

        // Run Agglomerative Hierarchical Clustering (AHC)
        let num_speakers = request.expected_speakers.unwrap_or(2).clamp(1, 10);
        let ahc = AgglomerativeClustering::new(0.35, Some(num_speakers));
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

        Ok(DiarizationResult::new(num_distinct_speakers, raw_turns))
    }
}
