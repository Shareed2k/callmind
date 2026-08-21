use crate::errors::DiarizationError;
use crate::models::{DiarizationRequest, DiarizationResult, SpeakerTurn};
use crate::traits::DiarizationEngine;
use async_trait::async_trait;
use callmind_core::SpeakerId;
use callmind_vad::{EnergyVadEngine, VadEngine};
use std::sync::Arc;

/// High-accuracy channel-based diarizer for stereo / dual-channel call recordings.
pub struct StereoChannelDiarizer {
    vad: Arc<dyn VadEngine>,
}

impl Default for StereoChannelDiarizer {
    fn default() -> Self {
        Self {
            vad: Arc::new(EnergyVadEngine::default()),
        }
    }
}

impl StereoChannelDiarizer {
    pub fn new(vad: Arc<dyn VadEngine>) -> Self {
        Self { vad }
    }
}

#[async_trait]
impl DiarizationEngine for StereoChannelDiarizer {
    async fn diarize(
        &self,
        request: DiarizationRequest<'_>,
    ) -> Result<DiarizationResult, DiarizationError> {
        let audio = request.audio;
        if audio.is_empty() {
            return Err(DiarizationError::EmptyAudio);
        }

        if audio.channels != 2 {
            return Err(DiarizationError::Inference(format!(
                "StereoChannelDiarizer requires 2 channels, but audio has {}",
                audio.channels
            )));
        }

        let (left_mono, right_mono) = audio
            .split_stereo()
            .map_err(|e| DiarizationError::Inference(e.to_string()))?;

        // Run VAD concurrently on left (Speaker 0) and right (Speaker 1)
        let (left_regions, right_regions) =
            tokio::try_join!(self.vad.detect(&left_mono), self.vad.detect(&right_mono))?;

        let mut turns = Vec::new();

        for region in left_regions {
            turns.push(SpeakerTurn::new(
                SpeakerId::new(0),
                region.start_ms,
                region.end_ms,
                Some(region.confidence),
            ));
        }

        for region in right_regions {
            turns.push(SpeakerTurn::new(
                SpeakerId::new(1),
                region.start_ms,
                region.end_ms,
                Some(region.confidence),
            ));
        }

        Ok(DiarizationResult::new(2, turns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use callmind_audio::AudioBuffer;

    #[tokio::test]
    async fn test_stereo_diarizer_separates_speakers() {
        let sample_rate = 16000;
        let mut stereo_samples = Vec::new();

        // 0 - 1.0s: Left speaker speaks (440Hz tone on Left, silence on Right)
        for i in 0..16000 {
            let t = (i as f32) / 16000.0;
            let val = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
            stereo_samples.push(val);
            stereo_samples.push(0.0001);
        }

        // 1.0 - 2.0s: Right speaker speaks (silence on Left, 800Hz tone on Right)
        for i in 0..16000 {
            let t = (i as f32) / 16000.0;
            let val = (2.0 * std::f32::consts::PI * 800.0 * t).sin() * 0.5;
            stereo_samples.push(0.0001);
            stereo_samples.push(val);
        }

        let audio = AudioBuffer::new(sample_rate, 2, stereo_samples);
        let diarizer = StereoChannelDiarizer::default();
        let result = diarizer
            .diarize(DiarizationRequest {
                audio: &audio,
                expected_speakers: Some(2),
            })
            .await
            .unwrap();

        assert_eq!(result.speakers, 2);
        assert!(!result.turns.is_empty());

        let spk0 = result.turns.iter().find(|t| t.speaker == SpeakerId::new(0));
        let spk1 = result.turns.iter().find(|t| t.speaker == SpeakerId::new(1));

        assert!(spk0.is_some());
        assert!(spk1.is_some());
        assert!(spk0.unwrap().start_ms < 1000);
        assert!(spk1.unwrap().start_ms >= 800);
    }
}
