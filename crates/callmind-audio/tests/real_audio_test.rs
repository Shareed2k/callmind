use callmind_audio::{AudioDecoder, AudioResampler, ChannelAnalyzer};
use callmind_vad::{EnergyVadEngine, VadEngine};
use std::path::Path;

#[tokio::test]
#[ignore = "Requires local /Volumes/calls dataset"]
async fn test_decode_multiple_real_calls_from_volume() {
    let call_files = [
        "/Volumes/calls/Call 033765660_260621_150956.m4a",
        "/Volumes/calls/Call recording סמי אינסטלטור_241229_091022.m4a",
        "/Volumes/calls/Call recording Шамиль Работа Bringg_240513_111449.m4a",
        "/Volumes/calls/Call recording Мама_240506_105603.m4a",
    ];

    for file_path in call_files {
        let p = Path::new(file_path);
        if !p.exists() {
            continue;
        }

        println!("\n>>> Testing real call: {:?}", p.file_name().unwrap());
        let decoded = AudioDecoder::decode_file(p).expect("Failed to decode real file");
        println!(
            "    Decoded: sample_rate={}Hz, channels={}, duration={:.2}s",
            decoded.sample_rate,
            decoded.channels,
            decoded.duration_ms() as f64 / 1000.0
        );

        let mode = ChannelAnalyzer::analyze(&decoded);
        println!("    Channel mode: {mode:?}");

        let resampled = AudioResampler::resample_to_16k_mono(&decoded).expect("Resampling failed");
        let vad = EnergyVadEngine::default();
        let regions = vad.detect(&resampled).await.expect("VAD failed");
        println!("    VAD detected {} speech regions", regions.len());

        assert!(decoded.duration_ms() > 0);
        assert!(!regions.is_empty());
    }
}
