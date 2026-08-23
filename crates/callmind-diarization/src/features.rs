use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};
use std::cell::RefCell;
use std::f32::consts::PI;
use std::sync::{Arc, OnceLock};

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

    /// Compute 80-channel log Mel filterbank energy frames from 16kHz audio.
    #[must_use]
    pub fn compute_fbank_80(samples: &[f32], sample_rate: u32) -> Vec<Vec<f32>> {
        if samples.len() < FRAME_SIZE {
            return Vec::new();
        }

        let mel_filters = build_mel_filterbank(sample_rate, FRAME_SIZE, 80);
        let mut frames = Vec::new();
        let mut start = 0;

        while start + FRAME_SIZE <= samples.len() {
            let frame = &samples[start..start + FRAME_SIZE];
            let mut windowed = Vec::with_capacity(FRAME_SIZE);
            for (i, &s) in frame.iter().enumerate() {
                let w = 0.54 - 0.46 * (2.0 * PI * i as f32 / (FRAME_SIZE as f32 - 1.0)).cos();
                windowed.push(s * w);
            }

            let spectrum = compute_power_spectrum(&windowed);
            let mut mel_energies = Vec::with_capacity(80);
            for filter in &mel_filters {
                let mut energy = 0.0f32;
                for (bin_idx, &weight) in filter.iter().enumerate() {
                    if bin_idx < spectrum.len() {
                        energy += spectrum[bin_idx] * weight;
                    }
                }
                mel_energies.push((energy.max(1e-6)).ln());
            }

            frames.push(mel_energies);
            start += FRAME_STEP;
        }

        // Per-utterance cepstral mean normalization, subtracting each mel band's
        // mean over time.
        //
        // Not optional: WeSpeaker and ECAPA-TDNN are both trained on kaldi fbank
        // with CMN applied, and the frame geometry above (400/160 at 16 kHz =
        // 25 ms / 10 ms) is already kaldi's. Feeding raw log-mel energies shifts
        // the input away from the training distribution, and the resulting
        // embeddings are dominated by recording level and channel rather than by
        // who is speaking -- measured as barely separating different people at
        // all (see `tests/embedding_sanity_probe.rs`).
        if let Some(width) = frames.first().map(Vec::len) {
            let count = frames.len() as f32;
            for band in 0..width {
                let mean = frames.iter().map(|f| f[band]).sum::<f32>() / count;
                for frame in &mut frames {
                    frame[band] -= mean;
                }
            }
        }

        frames
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

/// Cached forward real-FFT plan for `FRAME_SIZE`-length frames. Every hot call
/// site uses that one length, and planning costs far more than a transform.
fn frame_fft() -> &'static Arc<dyn RealToComplex<f32>> {
    static PLAN: OnceLock<Arc<dyn RealToComplex<f32>>> = OnceLock::new();
    PLAN.get_or_init(|| RealFftPlanner::<f32>::new().plan_fft_forward(FRAME_SIZE))
}

thread_local! {
    /// Reused input/output buffers, so the per-frame path stays allocation-free
    /// apart from the returned spectrum.
    static FFT_SCRATCH: RefCell<(Vec<f32>, Vec<Complex32>)> = RefCell::new((
        vec![0.0; FRAME_SIZE],
        vec![Complex32::new(0.0, 0.0); (FRAME_SIZE / 2) + 1],
    ));
}

/// Compute the power spectrum of one frame via a real FFT.
///
/// This was a hand-rolled O(n^2) DFT that called `cos` and `sin` once per
/// (bin, sample) pair: ~160k transcendental calls per 400-sample frame, at 100
/// frames per second of audio. Same transform, same `1/n` scaling, ~40x less
/// work.
fn compute_power_spectrum(frame: &[f32]) -> Vec<f32> {
    let n = frame.len();
    let num_bins = (n / 2) + 1;
    let scale = n as f32;

    // Only FRAME_SIZE has a cached plan. Other lengths are off the hot path, so
    // plan them on the spot rather than keeping a keyed cache alive.
    if n != FRAME_SIZE {
        let plan = RealFftPlanner::<f32>::new().plan_fft_forward(n);
        let mut input = frame.to_vec();
        let mut output = plan.make_output_vec();
        if plan.process(&mut input, &mut output).is_err() {
            return vec![0.0; num_bins];
        }
        return output.iter().map(|c| c.norm_sqr() / scale).collect();
    }

    FFT_SCRATCH.with(|cell| {
        let (input, output) = &mut *cell.borrow_mut();
        input.copy_from_slice(frame);
        if frame_fft().process(input, output).is_err() {
            return vec![0.0; num_bins];
        }
        output.iter().map(|c| c.norm_sqr() / scale).collect()
    })
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

    /// The previous hand-rolled O(n^2) DFT, kept only as a numerical oracle.
    fn naive_power_spectrum(frame: &[f32]) -> Vec<f32> {
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

    fn synth_frame(len: usize, seed_mix: u64) -> Vec<f32> {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ seed_mix;
        (0..len)
            .map(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let noise = ((seed >> 11) as f32 / (1u64 << 53) as f32) - 0.5;
                // Voiced-speech-like mix plus noise.
                (2.0 * PI * 140.0 * i as f32 / 16_000.0).sin() * 0.6
                    + (2.0 * PI * 430.0 * i as f32 / 16_000.0).sin() * 0.3
                    + noise * 0.1
            })
            .collect()
    }

    #[test]
    fn fft_matches_naive_dft() {
        for (case, frame) in [
            ("frame_size", synth_frame(FRAME_SIZE, 1)),
            ("off_hot_path_len", synth_frame(256, 2)),
            ("odd_len", synth_frame(257, 3)),
            ("silence", vec![0.0; FRAME_SIZE]),
            ("dc", vec![0.7; FRAME_SIZE]),
        ] {
            let got = compute_power_spectrum(&frame);
            let want = naive_power_spectrum(&frame);

            assert_eq!(got.len(), want.len(), "{case}: bin count changed");

            let peak = want.iter().copied().fold(0.0f32, f32::max).max(1e-6);
            for (bin, (g, w)) in got.iter().zip(want.iter()).enumerate() {
                // Relative to the spectrum peak: f32 summation order differs, so
                // exact equality is not achievable, but the transform is.
                assert!(
                    (g - w).abs() <= peak * 1e-4,
                    "{case}: bin {bin} diverged: fft={g} naive={w} peak={peak}"
                );
            }
        }
    }

    #[test]
    fn fft_scratch_is_reused_across_threads() {
        // The hot path keeps its buffers in a thread_local; make sure a second
        // thread initialises its own rather than panicking or sharing.
        let frame = synth_frame(FRAME_SIZE, 9);
        let expected = compute_power_spectrum(&frame);
        let handle = std::thread::spawn(move || compute_power_spectrum(&frame));
        assert_eq!(handle.join().unwrap(), expected);
    }

    #[test]
    #[ignore = "timing comparison, run with --ignored --nocapture"]
    fn perf_dft_old_vs_new() {
        let frames: Vec<Vec<f32>> = (0..200).map(|i| synth_frame(FRAME_SIZE, i)).collect();

        let t0 = std::time::Instant::now();
        for f in &frames {
            let _ = naive_power_spectrum(f);
        }
        let old = t0.elapsed();

        let t1 = std::time::Instant::now();
        for f in &frames {
            let _ = compute_power_spectrum(f);
        }
        let new = t1.elapsed();

        println!(
            "{} frames (2s of audio): old={old:?} new={new:?} speedup={:.0}x",
            frames.len(),
            old.as_secs_f64() / new.as_secs_f64().max(1e-9)
        );
    }
}
