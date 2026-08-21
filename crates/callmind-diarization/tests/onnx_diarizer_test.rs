use callmind_audio::AudioBuffer;
use callmind_diarization::{DiarizationEngine, DiarizationRequest, NeuralDiarizer};
use callmind_vad::EnergyVadEngine;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn test_neural_diarizer_fallback_when_model_missing() {
    let vad = Arc::new(EnergyVadEngine::default());
    let non_existent_model = PathBuf::from("./models/non_existent_embedding.onnx");

    // Should construct gracefully with fallback enabled
    let diarizer = NeuralDiarizer::new_with_fallback(Some(non_existent_model), vad);

    // Create 3 seconds of synthetic audio (1.5s 400Hz speaker A, 1.5s 800Hz speaker B)
    let sample_rate = 16000;
    let mut samples = Vec::with_capacity(sample_rate * 3);
    for i in 0..(sample_rate * 3) {
        let t = i as f32 / sample_rate as f32;
        let freq = if t < 1.5 { 400.0 } else { 800.0 };
        let amp = 0.5;
        let val = amp * (2.0 * std::f32::consts::PI * freq * t).sin();
        samples.push(val);
    }

    let audio = AudioBuffer::new(sample_rate as u32, 1, samples);
    let request = DiarizationRequest {
        audio: &audio,
        expected_speakers: Some(2),
    };

    let result = diarizer
        .diarize(request)
        .await
        .expect("diarization must succeed");
    assert!(!result.turns.is_empty(), "must produce speaker turns");
    assert!(result.speakers >= 1, "must identify speakers");
}
