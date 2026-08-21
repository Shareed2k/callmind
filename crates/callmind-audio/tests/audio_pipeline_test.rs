use callmind_audio::{AudioDecoder, AudioResampler, ChannelAnalyzer, ChannelMode};
use callmind_vad::{EnergyVadEngine, VadEngine};

/// Helper to generate a minimal valid WAV file in-memory.
fn generate_test_wav_bytes(sample_rate: u32, channels: u16, duration_secs: f32) -> Vec<u8> {
    let num_samples = (sample_rate as f32 * duration_secs) as usize * channels as usize;
    let mut pcm_data = Vec::with_capacity(num_samples);

    for i in 0..(num_samples / channels as usize) {
        let t = (i as f32) / (sample_rate as f32);
        let sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
        let sample_i16 = (sample * 32767.0) as i16;
        for _ in 0..channels {
            pcm_data.extend_from_slice(&sample_i16.to_le_bytes());
        }
    }

    let byte_rate = sample_rate * channels as u32 * 2;
    let block_align = channels * 2;
    let data_len = pcm_data.len() as u32;
    let riff_chunk_size = 36 + data_len;

    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_chunk_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    // fmt subchunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes()); // subchunk1size (16 for PCM)
    wav.extend_from_slice(&1_u16.to_le_bytes()); // audio format (1 = PCM)
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes()); // bits per sample

    // data subchunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&pcm_data);

    wav
}

#[tokio::test]
async fn test_full_audio_decode_resample_vad_pipeline() {
    // 1. Generate 44.1 kHz stereo WAV file with 2 seconds of 440Hz tone
    let wav_bytes = generate_test_wav_bytes(44100, 2, 2.0);

    // 2. Decode using Symphonia
    let decoded = AudioDecoder::decode_bytes(&wav_bytes, Some("wav")).unwrap();
    assert_eq!(decoded.sample_rate, 44100);
    assert_eq!(decoded.channels, 2);
    assert_eq!(decoded.duration_ms(), 2000);

    // 3. Channel Analysis (both channels have same tone -> StereoMixed)
    let channel_mode = ChannelAnalyzer::analyze(&decoded);
    assert_eq!(channel_mode, ChannelMode::StereoMixed);

    // 4. Resample to 16 kHz Mono
    let resampled = AudioResampler::resample_to_16k_mono(&decoded).unwrap();
    assert_eq!(resampled.sample_rate, 16000);
    assert_eq!(resampled.channels, 1);
    assert!((resampled.duration_ms() as i64 - 2000).abs() <= 50);

    // 5. Run VAD detection
    let vad = EnergyVadEngine::default();
    let regions = vad.detect(&resampled).await.unwrap();
    assert!(!regions.is_empty());
    assert!(regions[0].duration_ms() >= 1500);
}
