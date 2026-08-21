use crate::buffer::AudioBuffer;
use crate::errors::AudioError;

pub const STANDARD_STT_SAMPLE_RATE: u32 = 16000;

/// High-quality audio resampler converting audio to standard target sample rates (e.g., 16 kHz).
pub struct AudioResampler;

impl AudioResampler {
    /// Resample audio to 16,000 Hz Mono (the standard input format for Whisper STT and Diarization).
    pub fn resample_to_16k_mono(audio: &AudioBuffer) -> Result<AudioBuffer, AudioError> {
        let mono = audio.to_mono();
        if mono.sample_rate == STANDARD_STT_SAMPLE_RATE {
            return Ok(mono);
        }
        Self::resample(&mono, STANDARD_STT_SAMPLE_RATE)
    }

    /// Resample an `AudioBuffer` to the requested target sample rate using cubic / sinc interpolation.
    pub fn resample(
        audio: &AudioBuffer,
        target_sample_rate: u32,
    ) -> Result<AudioBuffer, AudioError> {
        if audio.is_empty() {
            return Err(AudioError::EmptyAudio);
        }
        if audio.sample_rate == target_sample_rate {
            return Ok(audio.clone());
        }

        let num_channels = audio.channels as usize;
        let from_rate = audio.sample_rate as f64;
        let to_rate = target_sample_rate as f64;
        let ratio = from_rate / to_rate;

        let total_frames = audio.total_frames();
        let target_frames = ((total_frames as f64) / ratio).round() as usize;

        let mut planar_channels: Vec<Vec<f32>> =
            vec![Vec::with_capacity(total_frames); num_channels];
        for frame_idx in 0..total_frames {
            for ch in 0..num_channels {
                planar_channels[ch].push(audio.samples[frame_idx * num_channels + ch]);
            }
        }

        let mut resampled_planar: Vec<Vec<f32>> = Vec::with_capacity(num_channels);
        for ch in &planar_channels {
            resampled_planar.push(cubic_resample(ch, target_frames, ratio));
        }

        let mut interleaved = Vec::with_capacity(target_frames * num_channels);
        for f in 0..target_frames {
            for ch in 0..num_channels {
                interleaved.push(resampled_planar[ch][f]);
            }
        }

        Ok(AudioBuffer::new(
            target_sample_rate,
            audio.channels,
            interleaved,
        ))
    }
}

/// High-quality Catmull-Rom cubic spline interpolation resampler.
fn cubic_resample(input: &[f32], target_frames: usize, ratio: f64) -> Vec<f32> {
    if input.is_empty() || target_frames == 0 {
        return Vec::new();
    }

    let mut output = Vec::with_capacity(target_frames);
    let input_len = input.len();

    for i in 0..target_frames {
        let src_idx = (i as f64) * ratio;
        let idx = src_idx.floor() as isize;
        let t = (src_idx - (idx as f64)) as f32;

        let get_sample = |j: isize| -> f32 {
            if j < 0 {
                input[0]
            } else if (j as usize) >= input_len {
                input[input_len - 1]
            } else {
                input[j as usize]
            }
        };

        let p0 = get_sample(idx - 1);
        let p1 = get_sample(idx);
        let p2 = get_sample(idx + 1);
        let p3 = get_sample(idx + 2);

        // Catmull-Rom spline formula
        let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
        let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
        let c = -0.5 * p0 + 0.5 * p2;
        let d = p1;

        let sample = a * t * t * t + b * t * t + c * t + d;
        output.push(sample.clamp(-1.0, 1.0));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_48k_to_16k() {
        // Generate 1 second of 480 Hz sine wave at 48 kHz
        let mut samples = Vec::with_capacity(48000);
        for i in 0..48000 {
            let t = (i as f32) / 48000.0;
            samples.push((2.0 * std::f32::consts::PI * 480.0 * t).sin());
        }

        let input_buf = AudioBuffer::new(48000, 1, samples);
        let resampled = AudioResampler::resample_to_16k_mono(&input_buf).unwrap();

        assert_eq!(resampled.sample_rate, 16000);
        assert_eq!(resampled.channels, 1);
        // Approximately 16000 frames for 1 second of audio
        assert!((resampled.samples.len() as i32 - 16000).abs() < 100);
    }
}
