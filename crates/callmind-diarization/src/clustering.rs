use crate::ahc::AgglomerativeClustering;
use crate::errors::DiarizationError;
use crate::features::AcousticFeatureExtractor;
use crate::models::{DiarizationRequest, DiarizationResult, SpeakerTurn};
use crate::traits::DiarizationEngine;
use async_trait::async_trait;
use callmind_core::SpeakerId;
use callmind_vad::{EnergyVadEngine, VadEngine};
use std::sync::Arc;

/// Acoustic voice embedding and AHC/K-Means clustering diarizer for Mono audio streams.
pub struct ClusteringDiarizer {
    vad: Arc<dyn VadEngine>,
}

impl Default for ClusteringDiarizer {
    fn default() -> Self {
        Self {
            vad: Arc::new(EnergyVadEngine::default()),
        }
    }
}

impl ClusteringDiarizer {
    pub fn new(vad: Arc<dyn VadEngine>) -> Self {
        Self { vad }
    }
}

#[async_trait]
impl DiarizationEngine for ClusteringDiarizer {
    async fn diarize(
        &self,
        request: DiarizationRequest<'_>,
    ) -> Result<DiarizationResult, DiarizationError> {
        let audio = request.audio;
        if audio.is_empty() {
            return Err(DiarizationError::EmptyAudio);
        }

        let mono = audio.to_mono();
        let regions = self.vad.detect(&mono).await?;

        if regions.is_empty() {
            return Ok(DiarizationResult::new(1, Vec::new()));
        }

        let k = request.expected_speakers.unwrap_or(2).clamp(1, 4);

        if regions.len() == 1 || k == 1 {
            let turns = regions
                .into_iter()
                .map(|r| {
                    SpeakerTurn::new(SpeakerId::new(0), r.start_ms, r.end_ms, Some(r.confidence))
                })
                .collect();
            return Ok(DiarizationResult::new(1, turns));
        }

        // 1. Generate sliding sub-segments for high temporal resolution (captures rapid back-and-forth turns)
        let sample_rate = mono.sample_rate as usize;
        let window_ms: u64 = 800;
        let hop_ms: u64 = 400;

        let mut sub_segments: Vec<(u64, u64)> = Vec::new();
        let mut sub_embeddings: Vec<Vec<f32>> = Vec::new();

        for region in &regions {
            let dur = region.duration_ms();
            if dur <= window_ms {
                let start_sample = (region.start_ms as usize * sample_rate) / 1000;
                let end_sample = (region.end_ms as usize * sample_rate) / 1000;
                let slice = &mono.samples[start_sample..end_sample.min(mono.samples.len())];
                let emb = AcousticFeatureExtractor::extract_embedding(slice, mono.sample_rate);
                sub_segments.push((region.start_ms, region.end_ms));
                sub_embeddings.push(emb);
            } else {
                let mut curr = region.start_ms;
                while curr + window_ms <= region.end_ms {
                    let s_end = (curr + window_ms).min(region.end_ms);
                    let start_sample = (curr as usize * sample_rate) / 1000;
                    let end_sample = (s_end as usize * sample_rate) / 1000;
                    let slice = &mono.samples[start_sample..end_sample.min(mono.samples.len())];
                    let emb = AcousticFeatureExtractor::extract_embedding(slice, mono.sample_rate);
                    sub_segments.push((curr, s_end));
                    sub_embeddings.push(emb);
                    curr += hop_ms;
                }
                if curr < region.end_ms {
                    let start_sample = (curr as usize * sample_rate) / 1000;
                    let end_sample = (region.end_ms as usize * sample_rate) / 1000;
                    let slice = &mono.samples[start_sample..end_sample.min(mono.samples.len())];
                    let emb = AcousticFeatureExtractor::extract_embedding(slice, mono.sample_rate);
                    sub_segments.push((curr, region.end_ms));
                    sub_embeddings.push(emb);
                }
            }
        }

        if sub_embeddings.is_empty() {
            return Ok(DiarizationResult::new(1, Vec::new()));
        }

        // 2. Perform Agglomerative Hierarchical Clustering (AHC)
        let ahc = AgglomerativeClustering::new(0.35, Some(k));
        let assignments = ahc.cluster(&sub_embeddings);

        // 3. Build merged SpeakerTurns from sub-segments
        let mut raw_turns: Vec<SpeakerTurn> = Vec::new();
        for ((start_ms, end_ms), &cluster_id) in sub_segments.into_iter().zip(assignments.iter()) {
            let spk_id = SpeakerId::new(cluster_id as u16);
            if let Some(last) = raw_turns.last_mut() {
                if last.speaker == spk_id && start_ms <= last.end_ms + 200 {
                    last.end_ms = last.end_ms.max(end_ms);
                    continue;
                }
            }
            raw_turns.push(SpeakerTurn::new(spk_id, start_ms, end_ms, Some(0.90)));
        }

        let num_distinct_speakers = raw_turns
            .iter()
            .map(|t| t.speaker.as_u16())
            .max()
            .map_or(1, |m| (m + 1) as usize);

        Ok(DiarizationResult::new(num_distinct_speakers, raw_turns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use callmind_audio::AudioBuffer;
    use std::f32::consts::PI;

    #[tokio::test]
    async fn test_clustering_diarizer_separates_different_pitches() {
        let sample_rate = 16000;
        let mut samples = Vec::new();

        // Speaker 0: Low fundamental pitch 120Hz (0.0s - 2.0s)
        for i in 0..32000 {
            let t = i as f32 / sample_rate as f32;
            samples.push(0.8 * (2.0 * PI * 120.0 * t).sin());
        }

        // Silence (2.0s - 3.0s)
        samples.extend(vec![0.0; 16000]);

        // Speaker 1: High fundamental pitch 280Hz (3.0s - 5.0s)
        for i in 0..32000 {
            let t = i as f32 / sample_rate as f32;
            samples.push(0.8 * (2.0 * PI * 280.0 * t).sin());
        }

        // Silence (5.0s - 6.0s)
        samples.extend(vec![0.0; 16000]);

        // Speaker 0 again: Low fundamental pitch 120Hz (6.0s - 8.0s)
        for i in 0..32000 {
            let t = i as f32 / sample_rate as f32;
            samples.push(0.8 * (2.0 * PI * 120.0 * t).sin());
        }

        let audio = AudioBuffer::new(sample_rate, 1, samples);
        let diarizer = ClusteringDiarizer::default();
        let result = diarizer
            .diarize(DiarizationRequest::new(&audio).with_expected_speakers(2))
            .await
            .unwrap();

        assert_eq!(
            result.turns.len(),
            3,
            "Should detect exactly 3 speech turns"
        );
        // Speaker in turn 0 and turn 2 (both low pitch) should be clustered to the same speaker ID!
        assert_eq!(
            result.turns[0].speaker, result.turns[2].speaker,
            "Turns 0 and 2 should share the same speaker cluster"
        );
        assert_ne!(
            result.turns[0].speaker, result.turns[1].speaker,
            "Turn 1 should be a different speaker cluster"
        );
    }
}
