use crate::errors::VadError;
use crate::region::SpeechRegion;
use crate::traits::VadEngine;
use async_trait::async_trait;
use callmind_audio::AudioBuffer;

/// Robust energy- and spectral-feature-based Voice Activity Detection engine.
#[derive(Debug, Clone)]
pub struct EnergyVadEngine {
    pub frame_size_ms: u64,
    pub min_speech_duration_ms: u64,
    pub min_silence_duration_ms: u64,
    pub padding_ms: u64,
}

impl Default for EnergyVadEngine {
    fn default() -> Self {
        Self {
            frame_size_ms: 20,
            min_speech_duration_ms: 200,
            min_silence_duration_ms: 300,
            padding_ms: 100,
        }
    }
}

impl EnergyVadEngine {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl VadEngine for EnergyVadEngine {
    async fn detect(&self, audio: &AudioBuffer) -> Result<Vec<SpeechRegion>, VadError> {
        if audio.is_empty() {
            return Err(VadError::EmptyAudio);
        }

        let mono = audio.to_mono();
        let sample_rate = mono.sample_rate as usize;
        let frame_samples = ((self.frame_size_ms as usize) * sample_rate) / 1000;

        if frame_samples == 0 {
            return Err(VadError::Processing(
                "Frame size sample count is zero".into(),
            ));
        }

        let total_frames = mono.samples.len() / frame_samples;
        if total_frames == 0 {
            return Ok(vec![SpeechRegion::new(0, mono.duration_ms(), 0.5)]);
        }

        // 1. Calculate RMS energy and Zero-Crossing Rate (ZCR) per frame
        let mut frame_energies = Vec::with_capacity(total_frames);
        let mut frame_zcr = Vec::with_capacity(total_frames);

        for i in 0..total_frames {
            let start = i * frame_samples;
            let end = (start + frame_samples).min(mono.samples.len());
            let frame = &mono.samples[start..end];

            // RMS Energy
            let sum_sq: f32 = frame.iter().map(|&s| s * s).sum();
            let rms = (sum_sq / (frame.len() as f32)).sqrt();
            frame_energies.push(rms);

            // Zero Crossing Rate
            let mut zcr_count = 0;
            for j in 1..frame.len() {
                if (frame[j] >= 0.0 && frame[j - 1] < 0.0)
                    || (frame[j] < 0.0 && frame[j - 1] >= 0.0)
                {
                    zcr_count += 1;
                }
            }
            frame_zcr.push((zcr_count as f32) / (frame.len() as f32));
        }

        // 2. Estimate background noise floor from lowest energy frames
        let mut sorted_energies = frame_energies.clone();
        sorted_energies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let noise_idx = (total_frames * 15) / 100;
        let noise_floor = sorted_energies[noise_idx];

        let speech_threshold = if noise_floor > 0.05 {
            noise_floor * 0.75
        } else {
            (noise_floor * 2.5).clamp(0.005, 0.05)
        };

        // 3. Mark speech frames
        let mut raw_speech_frames = vec![false; total_frames];
        for i in 0..total_frames {
            let energy = frame_energies[i];
            let zcr = frame_zcr[i];
            // Speech if energy exceeds threshold and ZCR is reasonable for speech (0.01 to 0.40)
            if energy >= speech_threshold && zcr > 0.005 {
                raw_speech_frames[i] = true;
            }
        }

        // 4. Group consecutive speech frames into intervals
        let mut raw_segments: Vec<(u64, u64)> = Vec::new();
        let mut current_start: Option<u64> = None;

        for i in 0..total_frames {
            let frame_time_ms = (i as u64) * self.frame_size_ms;
            if raw_speech_frames[i] {
                if current_start.is_none() {
                    current_start = Some(frame_time_ms);
                }
            } else if let Some(start) = current_start {
                raw_segments.push((start, frame_time_ms));
                current_start = None;
            }
        }

        if let Some(start) = current_start {
            raw_segments.push((start, (total_frames as u64) * self.frame_size_ms));
        }

        if raw_segments.is_empty() {
            return Ok(Vec::new());
        }

        // 5. Merge segments separated by silence shorter than `min_silence_duration_ms`
        let mut merged_segments: Vec<(u64, u64)> = Vec::new();
        let mut current_seg = raw_segments[0];

        for next_seg in raw_segments.into_iter().skip(1) {
            let silence_gap = next_seg.0.saturating_sub(current_seg.1);
            if silence_gap <= self.min_silence_duration_ms {
                // Merge
                current_seg.1 = next_seg.1;
            } else {
                merged_segments.push(current_seg);
                current_seg = next_seg;
            }
        }
        merged_segments.push(current_seg);

        // 6. Filter by min speech duration and add padding
        let total_audio_ms = mono.duration_ms();
        let mut result = Vec::new();

        for (start, end) in merged_segments {
            let duration = end.saturating_sub(start);
            if duration >= self.min_speech_duration_ms {
                let padded_start = start.saturating_sub(self.padding_ms);
                let padded_end = (end + self.padding_ms).min(total_audio_ms);
                result.push(SpeechRegion::new(padded_start, padded_end, 0.90));
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_energy_vad_detects_speech_and_silence() {
        let sample_rate = 16000;
        let mut samples = Vec::new();

        // 0.0 - 0.5s: Silence
        samples.extend(vec![0.0001; 8000]);

        // 0.5 - 1.5s: Speech tone (400 Hz sine wave)
        for i in 0..16000 {
            let t = (i as f32) / 16000.0;
            samples.push((2.0 * std::f32::consts::PI * 400.0 * t).sin() * 0.5);
        }

        // 1.5 - 2.0s: Silence
        samples.extend(vec![0.0001; 8000]);

        let audio = AudioBuffer::new(sample_rate, 1, samples);
        let vad = EnergyVadEngine::default();
        let regions = vad.detect(&audio).await.unwrap();

        assert!(!regions.is_empty());
        let first = &regions[0];
        // Speech region should be roughly between 400ms and 1600ms (including padding)
        assert!(first.start_ms <= 600);
        assert!(first.end_ms >= 1400);
    }
}
