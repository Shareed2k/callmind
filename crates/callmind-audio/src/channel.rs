use crate::buffer::AudioBuffer;
use serde::{Deserialize, Serialize};

/// Channel mode classification for call audio streams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelMode {
    Mono,
    StereoMixed,
    StereoSeparated {
        left_channel: usize,
        right_channel: usize,
    },
    MultiChannel(u16),
}

/// Helper for analyzing channel correlation and identifying speaker separation.
pub struct ChannelAnalyzer;

impl ChannelAnalyzer {
    /// Analyze an `AudioBuffer` to determine channel mode and whether ML diarization is needed.
    pub fn analyze(audio: &AudioBuffer) -> ChannelMode {
        if audio.channels == 1 {
            return ChannelMode::Mono;
        }
        if audio.channels > 2 {
            return ChannelMode::MultiChannel(audio.channels);
        }

        if let Ok((left, right)) = audio.split_stereo() {
            let left_rms = left.rms();
            let right_rms = right.rms();

            // If one channel is virtually silent, it's not separated stereo
            if left_rms < 0.001 || right_rms < 0.001 {
                return ChannelMode::StereoMixed;
            }

            // Calculate Pearson correlation coefficient between Left and Right channels
            let correlation = pearson_correlation(&left.samples, &right.samples);

            // If correlation is high (> 0.85), both channels have the same mixed audio
            if correlation > 0.85 {
                ChannelMode::StereoMixed
            } else {
                // Low correlation -> Separated stereo channels
                ChannelMode::StereoSeparated {
                    left_channel: 0,
                    right_channel: 1,
                }
            }
        } else {
            ChannelMode::Mono
        }
    }
}

/// Calculate Pearson correlation coefficient between two audio sample slices.
fn pearson_correlation(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }

    let mean_a: f32 = a[..len].iter().sum::<f32>() / (len as f32);
    let mean_b: f32 = b[..len].iter().sum::<f32>() / (len as f32);

    let mut num = 0.0;
    let mut denom_a = 0.0;
    let mut denom_b = 0.0;

    for i in 0..len {
        let diff_a = a[i] - mean_a;
        let diff_b = b[i] - mean_b;
        num += diff_a * diff_b;
        denom_a += diff_a * diff_a;
        denom_b += diff_b * diff_b;
    }

    let denom = (denom_a * denom_b).sqrt();
    if denom < 1e-6 {
        let mean_abs_diff: f32 = a[..len]
            .iter()
            .zip(&b[..len])
            .map(|(&x, &y)| (x - y).abs())
            .sum::<f32>()
            / (len as f32);
        if mean_abs_diff < 1e-4 { 1.0 } else { 0.0 }
    } else {
        (num / denom).clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_analysis_mono_and_stereo() {
        let mono_buf = AudioBuffer::new(16000, 1, vec![0.1; 16000]);
        assert_eq!(ChannelAnalyzer::analyze(&mono_buf), ChannelMode::Mono);

        // Identical stereo channels -> StereoMixed
        let mut mixed_samples = Vec::new();
        for _ in 0..1000 {
            mixed_samples.push(0.5);
            mixed_samples.push(0.5);
        }
        let mixed_buf = AudioBuffer::new(16000, 2, mixed_samples);
        assert_eq!(
            ChannelAnalyzer::analyze(&mixed_buf),
            ChannelMode::StereoMixed
        );

        // Independent stereo channels -> StereoSeparated
        let mut separated_samples = Vec::new();
        for i in 0..1000 {
            let left_val = (i as f32 * 0.05).sin();
            let right_val = (i as f32 * 0.13).cos();
            separated_samples.push(left_val);
            separated_samples.push(right_val);
        }
        let separated_buf = AudioBuffer::new(16000, 2, separated_samples);
        assert!(matches!(
            ChannelAnalyzer::analyze(&separated_buf),
            ChannelMode::StereoSeparated {
                left_channel: 0,
                right_channel: 1
            }
        ));
    }
}
