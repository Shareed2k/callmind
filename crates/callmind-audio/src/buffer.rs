use crate::errors::AudioError;
use serde::{Deserialize, Serialize};

/// In-memory decoded PCM f32 audio buffer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl AudioBuffer {
    /// Create a new `AudioBuffer`.
    pub fn new(sample_rate: u32, channels: u16, samples: Vec<f32>) -> Self {
        assert!(channels > 0, "Channels must be >= 1");
        Self {
            sample_rate,
            channels,
            samples,
        }
    }

    /// Return total audio frames (samples per channel).
    pub fn total_frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / (self.channels as usize)
        }
    }

    /// Return audio duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 || self.channels == 0 {
            return 0;
        }
        let frames = self.total_frames() as u64;
        (frames * 1000) / (self.sample_rate as u64)
    }

    /// Check if buffer contains any audio samples.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Downmix multi-channel audio to single-channel (Mono) by averaging channels.
    #[must_use]
    pub fn to_mono(&self) -> Self {
        if self.channels == 1 {
            return self.clone();
        }

        let num_channels = self.channels as usize;
        let total_frames = self.total_frames();
        let mut mono_samples = Vec::with_capacity(total_frames);

        for frame_idx in 0..total_frames {
            let start = frame_idx * num_channels;
            let sum: f32 = self.samples[start..start + num_channels].iter().sum();
            mono_samples.push(sum / (num_channels as f32));
        }

        Self {
            sample_rate: self.sample_rate,
            channels: 1,
            samples: mono_samples,
        }
    }

    /// Split a stereo buffer into two mono buffers (Channel 0 / Left and Channel 1 / Right).
    pub fn split_stereo(&self) -> Result<(Self, Self), AudioError> {
        if self.channels != 2 {
            return Err(AudioError::Channel(format!(
                "Cannot split stereo buffer: audio has {} channels (expected 2)",
                self.channels
            )));
        }

        let total_frames = self.total_frames();
        let mut left_samples = Vec::with_capacity(total_frames);
        let mut right_samples = Vec::with_capacity(total_frames);

        let mut i = 0;
        while i + 1 < self.samples.len() {
            left_samples.push(self.samples[i]);
            right_samples.push(self.samples[i + 1]);
            i += 2;
        }

        let left = Self {
            sample_rate: self.sample_rate,
            channels: 1,
            samples: left_samples,
        };

        let right = Self {
            sample_rate: self.sample_rate,
            channels: 1,
            samples: right_samples,
        };

        Ok((left, right))
    }

    /// Extract a specific channel (0-indexed) as a new mono `AudioBuffer`.
    pub fn extract_channel(&self, channel_idx: usize) -> Result<Self, AudioError> {
        if channel_idx >= self.channels as usize {
            return Err(AudioError::Channel(format!(
                "Channel index {channel_idx} out of range (total channels: {})",
                self.channels
            )));
        }

        let num_channels = self.channels as usize;
        let total_frames = self.total_frames();
        let mut channel_samples = Vec::with_capacity(total_frames);

        for frame_idx in 0..total_frames {
            channel_samples.push(self.samples[frame_idx * num_channels + channel_idx]);
        }

        Ok(Self {
            sample_rate: self.sample_rate,
            channels: 1,
            samples: channel_samples,
        })
    }

    /// Calculate the Root Mean Square (RMS) energy of the audio.
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = self.samples.iter().map(|&s| s * s).sum();
        (sum_sq / (self.samples.len() as f32)).sqrt()
    }

    /// Extract a sub-segment of audio between `start_ms` and `end_ms`.
    #[must_use]
    pub fn slice_time(&self, start_ms: u64, end_ms: u64) -> Self {
        if start_ms >= end_ms || self.sample_rate == 0 || self.channels == 0 {
            return Self::new(self.sample_rate, self.channels, Vec::new());
        }

        let start_frame = ((start_ms as usize) * (self.sample_rate as usize)) / 1000;
        let end_frame = ((end_ms as usize) * (self.sample_rate as usize)) / 1000;

        let num_channels = self.channels as usize;
        let start_sample = (start_frame * num_channels).min(self.samples.len());
        let end_sample = (end_frame * num_channels).min(self.samples.len());

        let sliced_samples = self.samples[start_sample..end_sample].to_vec();

        Self {
            sample_rate: self.sample_rate,
            channels: self.channels,
            samples: sliced_samples,
        }
    }

    /// Encode the audio buffer into a standard 16-bit Linear PCM RIFF/WAVE byte vector.
    #[must_use]
    pub fn to_wav_bytes(&self) -> Vec<u8> {
        let num_samples = self.samples.len();
        let data_len = (num_samples * 2) as u32;
        let file_size_minus_8 = 36 + data_len;
        let byte_rate = self.sample_rate * (self.channels as u32) * 2;
        let block_align = self.channels * 2;

        let mut wav = Vec::with_capacity(44 + (data_len as usize));

        // RIFF header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&file_size_minus_8.to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        // fmt subchunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // Subchunk1Size = 16 for PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // AudioFormat = 1 (PCM)
        wav.extend_from_slice(&self.channels.to_le_bytes());
        wav.extend_from_slice(&self.sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes()); // BitsPerSample = 16

        // data subchunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());

        // 16-bit PCM samples
        for &sample in &self.samples {
            let clamped = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
            wav.extend_from_slice(&clamped.to_le_bytes());
        }

        wav
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_buffer_mono_and_duration() {
        // 1 second of 16kHz stereo audio
        let stereo_samples = vec![0.5, -0.5, 1.0, 0.0];
        let buf = AudioBuffer::new(16000, 2, stereo_samples);

        assert_eq!(buf.total_frames(), 2);
        let mono = buf.to_mono();
        assert_eq!(mono.channels, 1);
        assert_eq!(mono.samples, vec![0.0, 0.5]);
    }

    #[test]
    fn test_split_stereo() {
        let stereo_samples = vec![0.1, 0.9, 0.2, 0.8, 0.3, 0.7];
        let buf = AudioBuffer::new(16000, 2, stereo_samples);

        let (left, right) = buf.split_stereo().unwrap();
        assert_eq!(left.samples, vec![0.1, 0.2, 0.3]);
        assert_eq!(right.samples, vec![0.9, 0.8, 0.7]);
    }

    #[test]
    fn test_to_wav_bytes() {
        let samples = vec![0.0, 0.5, -0.5, 1.0];
        let buf = AudioBuffer::new(16000, 1, samples);
        let wav = buf.to_wav_bytes();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 8);
    }
}
