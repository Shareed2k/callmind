use crate::buffer::AudioBuffer;
use crate::errors::AudioError;
use rubato::{FftFixedInOut, Resampler};

pub const STANDARD_STT_SAMPLE_RATE: u32 = 16000;

/// Desired input frames per resampler call. Chunking only; it does not change
/// the output. `FftFixedInOut` may round it up to a valid size.
const RESAMPLER_CHUNK_FRAMES: usize = 1024;

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

    /// Resample an `AudioBuffer` to the requested target sample rate.
    ///
    /// Uses `rubato`'s FFT resampler, which band-limits the signal before
    /// decimating. The previous hand-rolled Catmull-Rom interpolation had no
    /// anti-aliasing filter at all: going 48 kHz -> 16 kHz drops Nyquist from
    /// 24 kHz to 8 kHz, so every component above 8 kHz folded straight back
    /// into the speech band that Whisper reads.
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
        let total_frames = audio.total_frames();
        let target_frames = ((total_frames as f64) * f64::from(target_sample_rate)
            / f64::from(audio.sample_rate))
        .round() as usize;

        let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(total_frames); num_channels];
        for frame_idx in 0..total_frames {
            for ch in 0..num_channels {
                planar[ch].push(audio.samples[frame_idx * num_channels + ch]);
            }
        }

        let mut resampler = FftFixedInOut::<f32>::new(
            audio.sample_rate as usize,
            target_sample_rate as usize,
            RESAMPLER_CHUNK_FRAMES,
            num_channels,
        )
        .map_err(|e| AudioError::Resample(e.to_string()))?;

        // Band-limiting costs a fixed latency. Drop it from the front so
        // word-level timestamps stay aligned with the source audio.
        let delay = resampler.output_delay();
        let wanted = target_frames + delay;

        let mut out_planar: Vec<Vec<f32>> = vec![Vec::with_capacity(wanted); num_channels];
        let mut pos = 0usize;

        while out_planar[0].len() < wanted {
            let need = resampler.input_frames_next();

            let produced = if pos + need <= total_frames {
                let chunk: Vec<&[f32]> = planar.iter().map(|c| &c[pos..pos + need]).collect();
                pos += need;
                resampler.process(&chunk, None)
            } else if pos < total_frames {
                let chunk: Vec<&[f32]> = planar.iter().map(|c| &c[pos..]).collect();
                pos = total_frames;
                resampler.process_partial(Some(&chunk), None)
            } else {
                // Input exhausted: flush the filter tail with silence.
                resampler.process_partial::<&[f32]>(None, None)
            }
            .map_err(|e| AudioError::Resample(e.to_string()))?;

            let gained = produced.first().map_or(0, Vec::len);
            for (ch, frames) in produced.into_iter().enumerate() {
                out_planar[ch].extend(frames);
            }

            // A flush that yields nothing would otherwise spin forever.
            if gained == 0 && pos >= total_frames {
                break;
            }
        }

        // Trim the latency and pin the length, padding with silence if the
        // flush came up short.
        let mut interleaved = Vec::with_capacity(target_frames * num_channels);
        for f in 0..target_frames {
            for ch in 0..num_channels {
                interleaved.push(out_planar[ch].get(delay + f).copied().unwrap_or(0.0));
            }
        }

        Ok(AudioBuffer::new(
            target_sample_rate,
            audio.channels,
            interleaved,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn tone(freq: f32, sample_rate: u32, secs: f32) -> AudioBuffer {
        let n = (sample_rate as f32 * secs) as usize;
        let samples = (0..n)
            .map(|i| (2.0 * PI * freq * (i as f32) / (sample_rate as f32)).sin())
            .collect();
        AudioBuffer::new(sample_rate, 1, samples)
    }

    /// Amplitude at one frequency, via direct correlation against a complex
    /// exponential (a single-bin DFT).
    fn amplitude_at(samples: &[f32], freq: f32, sample_rate: u32) -> f32 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, &s) in samples.iter().enumerate() {
            let angle = 2.0 * PI * freq * (i as f32) / (sample_rate as f32);
            re += s * angle.cos();
            im -= s * angle.sin();
        }
        2.0 * (re * re + im * im).sqrt() / (samples.len() as f32)
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / (samples.len() as f32)).sqrt()
    }

    #[test]
    fn test_resample_48k_to_16k() {
        let input_buf = tone(480.0, 48000, 1.0);
        let resampled = AudioResampler::resample_to_16k_mono(&input_buf).unwrap();

        assert_eq!(resampled.sample_rate, 16000);
        assert_eq!(resampled.channels, 1);
        assert!((resampled.samples.len() as i32 - 16000).abs() < 100);
    }

    /// The whole point of the rewrite. A 12 kHz tone cannot be represented at
    /// 16 kHz (Nyquist 8 kHz). Without a band-limiting filter it folds down to
    /// |16000 - 12000| = 4 kHz at nearly full amplitude and lands right in the
    /// speech band Whisper reads.
    #[test]
    fn ultrasonic_tone_is_filtered_not_aliased() {
        let input = tone(12_000.0, 48000, 0.5);
        let out = AudioResampler::resample_to_16k_mono(&input).unwrap();

        let alias = amplitude_at(&out.samples, 4_000.0, 16_000);
        assert!(
            alias < 0.05,
            "12 kHz folded back as a {alias:.3} amplitude alias at 4 kHz"
        );
        assert!(
            rms(&out.samples) < 0.05,
            "out-of-band energy survived: rms {:.3}",
            rms(&out.samples)
        );
    }

    /// Filtering must not eat the speech band it is protecting.
    #[test]
    fn speech_band_passes_through() {
        for freq in [300.0f32, 1_000.0, 3_000.0] {
            let input = tone(freq, 48000, 0.5);
            let out = AudioResampler::resample_to_16k_mono(&input).unwrap();
            let amp = amplitude_at(&out.samples, freq, 16_000);
            assert!(
                amp > 0.85,
                "{freq} Hz was attenuated to {amp:.3}; the passband must stay flat"
            );
        }
    }

    /// The FFT resampler has a fixed latency. It is compensated, otherwise every
    /// word-level timestamp would drift by that amount.
    #[test]
    fn transient_keeps_its_position() {
        let sample_rate = 48_000;
        let mut samples = vec![0.0f32; sample_rate as usize];
        // A short burst starting exactly 500 ms in.
        let burst_start = sample_rate as usize / 2;
        for i in 0..(sample_rate as usize / 100) {
            let t = (i as f32) / (sample_rate as f32);
            samples[burst_start + i] = (2.0 * PI * 1000.0 * t).sin();
        }

        let out =
            AudioResampler::resample(&AudioBuffer::new(sample_rate, 1, samples), 16_000).unwrap();

        let peak = out
            .samples
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map_or(0, |(i, _)| i);
        let peak_ms = (peak as f64) * 1000.0 / 16_000.0;
        assert!(
            (peak_ms - 500.0).abs() < 15.0,
            "burst moved to {peak_ms:.1}ms; expected ~500ms, so the resampler delay is not compensated"
        );
    }

    #[test]
    fn preserves_duration_across_rates() {
        for rate in [8_000u32, 22_050, 44_100, 48_000] {
            let input = tone(440.0, rate, 1.0);
            let out = AudioResampler::resample_to_16k_mono(&input).unwrap();
            assert_eq!(out.sample_rate, 16_000);
            let drift = (out.duration_ms() as i64 - 1000).abs();
            assert!(drift < 20, "{rate} Hz -> 16 kHz drifted {drift}ms");
        }
    }
}
