use std::f32::consts::PI;

/// Dimension of the acoustic voice embedding vector.
pub const EMBEDDING_DIM: usize = 20;

/// Number of Mel filterbanks.
const NUM_MEL_FILTERS: usize = 20;

/// Frame size in samples for 16kHz audio (25ms = 400 samples).
const FRAME_SIZE: usize = 400;

/// Frame step in samples for 16kHz audio (10ms = 160 samples).
const FRAME_STEP: usize = 160;

/// Acoustic voice feature extractor for speaker diarization.
pub struct AcousticFeatureExtractor;

impl AcousticFeatureExtractor {
    /// Extract a normalized acoustic voice embedding vector from raw 16kHz mono audio samples.
    #[must_use]
    pub fn extract_embedding(samples: &[f32], sample_rate: u32) -> Vec<f32> {
        if samples.len() < FRAME_SIZE {
            return vec![0.0; EMBEDDING_DIM];
        }

        let mel_filters = build_mel_filterbank(sample_rate, FRAME_SIZE, NUM_MEL_FILTERS);
        let mut mfcc_sum = [0.0f32; NUM_MEL_FILTERS];
        let mut num_frames = 0;

        let mut pitch_sum = 0.0f32;
        let mut pitch_count = 0;

        let mut spectral_centroid_sum = 0.0f32;

        let mut start = 0;
        while start + FRAME_SIZE <= samples.len() {
            let frame = &samples[start..start + FRAME_SIZE];

            // 1. Apply Hamming window
            let mut windowed = Vec::with_capacity(FRAME_SIZE);
            for (i, &s) in frame.iter().enumerate() {
                let w = 0.54 - 0.46 * (2.0 * PI * i as f32 / (FRAME_SIZE as f32 - 1.0)).cos();
                windowed.push(s * w);
            }

            // 2. Magnitude power spectrum via DFT
            let spectrum = compute_power_spectrum(&windowed);

            // 3. Mel Filterbank energies
            let mut mel_energies = Vec::with_capacity(NUM_MEL_FILTERS);
            for filter in &mel_filters {
                let mut energy = 0.0f32;
                for (bin_idx, &weight) in filter.iter().enumerate() {
                    if bin_idx < spectrum.len() {
                        energy += spectrum[bin_idx] * weight;
                    }
                }
                mel_energies.push((energy.max(1e-6)).ln());
            }

            // 4. Discrete Cosine Transform (DCT-II)
            for k in 0..NUM_MEL_FILTERS {
                let mut sum = 0.0f32;
                for (n, &e) in mel_energies.iter().enumerate() {
                    sum +=
                        e * (PI * (k as f32) * (n as f32 + 0.5) / (NUM_MEL_FILTERS as f32)).cos();
                }
                mfcc_sum[k] += sum;
            }

            // 5. Pitch estimation via Autocorrelation (range: 70Hz - 350Hz)
            if let Some(pitch) = estimate_pitch(frame, sample_rate) {
                pitch_sum += pitch;
                pitch_count += 1;
            }

            // 6. Spectral centroid
            let mut num = 0.0f32;
            let mut den = 0.0f32;
            for (idx, &mag) in spectrum.iter().enumerate() {
                let freq = (idx as f32 * sample_rate as f32) / (FRAME_SIZE as f32);
                num += freq * mag;
                den += mag;
            }
            if den > 1e-6 {
                spectral_centroid_sum += num / den;
            }

            num_frames += 1;
            start += FRAME_STEP;
        }

        if num_frames == 0 {
            return vec![0.0; EMBEDDING_DIM];
        }

        let mut embedding = Vec::with_capacity(EMBEDDING_DIM);
        for k in 0..NUM_MEL_FILTERS.min(EMBEDDING_DIM - 2) {
            embedding.push(mfcc_sum[k] / num_frames as f32);
        }

        // Normalized average pitch (scaled to ~0.0-1.0 range)
        let avg_pitch = if pitch_count > 0 {
            (pitch_sum / pitch_count as f32) / 400.0
        } else {
            0.3
        };
        embedding.push(avg_pitch);

        // Normalized spectral centroid
        let avg_centroid = (spectral_centroid_sum / num_frames as f32) / 4000.0;
        embedding.push(avg_centroid);

        // L2 normalization
        let norm_sq: f32 = embedding.iter().map(|&x| x * x).sum();
        let norm = norm_sq.sqrt();
        if norm > 1e-6 {
            for x in &mut embedding {
                *x /= norm;
            }
        }

        embedding
    }

    /// Calculate cosine distance between two embedding vectors (0.0 = identical, 1.0 = orthogonal, 2.0 = opposite).
    #[must_use]
    pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 1.0;
        }

        let mut dot = 0.0f32;
        let mut norm_a_sq = 0.0f32;
        let mut norm_b_sq = 0.0f32;

        for (&x, &y) in a.iter().zip(b.iter()) {
            dot += x * y;
            norm_a_sq += x * x;
            norm_b_sq += y * y;
        }

        let denom = (norm_a_sq * norm_b_sq).sqrt();
        if denom < 1e-6 {
            1.0
        } else {
            (1.0 - (dot / denom)).clamp(0.0, 2.0)
        }
    }
}

/// Compute power spectrum using simplified real DFT.
fn compute_power_spectrum(frame: &[f32]) -> Vec<f32> {
    let n = frame.len();
    let num_bins = (n / 2) + 1;
    let mut power = Vec::with_capacity(num_bins);

    for k in 0..num_bins {
        let mut real = 0.0f32;
        let mut imag = 0.0f32;
        let angle_step = 2.0 * PI * (k as f32) / (n as f32);

        for (t, &val) in frame.iter().enumerate() {
            let angle = angle_step * (t as f32);
            real += val * angle.cos();
            imag -= val * angle.sin();
        }

        power.push((real * real + imag * imag) / (n as f32));
    }

    power
}

/// Convert frequency in Hz to Mel scale.
fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

/// Convert Mel scale to frequency in Hz.
fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10.0f32.powf(mel / 2595.0) - 1.0)
}

/// Build triangular Mel filterbanks.
fn build_mel_filterbank(sample_rate: u32, frame_size: usize, num_filters: usize) -> Vec<Vec<f32>> {
    let low_freq = 100.0f32;
    let high_freq = (sample_rate as f32) / 2.0;

    let low_mel = hz_to_mel(low_freq);
    let high_mel = hz_to_mel(high_freq);

    let num_points = num_filters + 2;
    let mel_step = (high_mel - low_mel) / ((num_points - 1) as f32);

    let mut bin_indices = Vec::with_capacity(num_points);
    for i in 0..num_points {
        let mel = low_mel + i as f32 * mel_step;
        let hz = mel_to_hz(mel);
        let bin = ((frame_size + 1) as f32 * hz / sample_rate as f32).floor() as usize;
        bin_indices.push(bin.min(frame_size / 2));
    }

    let num_bins = (frame_size / 2) + 1;
    let mut filterbank = Vec::with_capacity(num_filters);

    for m in 1..=num_filters {
        let mut filter = vec![0.0f32; num_bins];
        let f_m_minus = bin_indices[m - 1];
        let f_m = bin_indices[m];
        let f_m_plus = bin_indices[m + 1];

        for k in f_m_minus..f_m {
            if f_m > f_m_minus {
                filter[k] = (k - f_m_minus) as f32 / (f_m - f_m_minus) as f32;
            }
        }
        for k in f_m..f_m_plus {
            if f_m_plus > f_m {
                filter[k] = (f_m_plus - k) as f32 / (f_m_plus - f_m) as f32;
            }
        }

        filterbank.push(filter);
    }

    filterbank
}

/// Estimate pitch using normalized autocorrelation within 70Hz - 350Hz range.
fn estimate_pitch(frame: &[f32], sample_rate: u32) -> Option<f32> {
    let min_lag = (sample_rate as f32 / 350.0).floor() as usize; // ~45 samples at 16kHz
    let max_lag = (sample_rate as f32 / 70.0).ceil() as usize; // ~228 samples at 16kHz

    if max_lag >= frame.len() {
        return None;
    }

    let mut max_corr = 0.0f32;
    let mut best_lag = 0;

    let energy: f32 = frame.iter().map(|&s| s * s).sum();
    if energy < 1e-4 {
        return None;
    }

    for lag in min_lag..=max_lag {
        let mut corr = 0.0f32;
        for i in 0..(frame.len() - lag) {
            corr += frame[i] * frame[i + lag];
        }
        let norm_corr = corr / energy;
        if norm_corr > max_corr && norm_corr > 0.35 {
            max_corr = norm_corr;
            best_lag = lag;
        }
    }

    if best_lag > 0 {
        Some(sample_rate as f32 / best_lag as f32)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature_extraction_and_cosine_distance() {
        // Generate two different sine waves (different pitch / frequencies)
        let sample_rate = 16000;
        let mut voice1 = Vec::new();
        let mut voice2 = Vec::new();

        for i in 0..16000 {
            // Speaker 1: Low fundamental pitch ~120 Hz with harmonics
            let t = i as f32 / sample_rate as f32;
            voice1.push((2.0 * PI * 120.0 * t).sin() + 0.5 * (2.0 * PI * 240.0 * t).sin());

            // Speaker 2: High fundamental pitch ~240 Hz with harmonics
            voice2.push((2.0 * PI * 240.0 * t).sin() + 0.5 * (2.0 * PI * 480.0 * t).sin());
        }

        let emb1_a = AcousticFeatureExtractor::extract_embedding(&voice1[0..8000], sample_rate);
        let emb1_b = AcousticFeatureExtractor::extract_embedding(&voice1[8000..16000], sample_rate);
        let emb2 = AcousticFeatureExtractor::extract_embedding(&voice2, sample_rate);

        // Self-similarity of Speaker 1 across chunks should have small distance
        let dist_same_speaker = AcousticFeatureExtractor::cosine_distance(&emb1_a, &emb1_b);
        assert!(
            dist_same_speaker < 0.15,
            "Same voice distance should be small: {dist_same_speaker}"
        );

        // Distance between Speaker 1 and Speaker 2 should be significantly larger
        let dist_different_speakers = AcousticFeatureExtractor::cosine_distance(&emb1_a, &emb2);
        assert!(
            dist_different_speakers > dist_same_speaker,
            "Different voices must have higher distance"
        );
    }
}
