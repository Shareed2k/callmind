use callmind_audio::{AudioDecoder, AudioResampler};
use std::path::Path;

#[tokio::test]
#[ignore = "Requires local audio and Whisper GPU weights"]
async fn test_whisper_real_hebrew_transcription() {
    let call_path = if Path::new("./data/recordings/00000000-0000-0000-0000-000000000001/0064c18a-cc5e-48fb-9e0b-9da19d78e49f.m4a").exists() {
        Path::new("./data/recordings/00000000-0000-0000-0000-000000000001/0064c18a-cc5e-48fb-9e0b-9da19d78e49f.m4a")
    } else if Path::new("./data/recordings/00000000-0000-0000-0000-000000000001/0064c18a-cc5e-48fb-9e0b-9da19d78e49f.wav").exists() {
        Path::new("./data/recordings/00000000-0000-0000-0000-000000000001/0064c18a-cc5e-48fb-9e0b-9da19d78e49f.wav")
    } else {
        Path::new("../../data/recordings/00000000-0000-0000-0000-000000000001/0064c18a-cc5e-48fb-9e0b-9da19d78e49f.m4a")
    };

    let model_path = if Path::new("./models/stt/ivrit-ai-large-v3-turbo.bin").exists() {
        Path::new("./models/stt/ivrit-ai-large-v3-turbo.bin")
    } else {
        Path::new("../../models/stt/ivrit-ai-large-v3-turbo.bin")
    };

    if !call_path.exists() || !model_path.exists() {
        println!("Skipping: audio or model missing");
        return;
    }

    let decoded = AudioDecoder::decode_file(call_path).unwrap();
    let resampled = AudioResampler::resample_to_16k_mono(&decoded).unwrap();

    let model_str = model_path.to_string_lossy().to_string();
    let audio_samples = resampled.samples;

    let (text, detected_lang) = tokio::task::spawn_blocking(move || {
        let ctx = whisper_rs::WhisperContext::new_with_params(
            &model_str,
            whisper_rs::WhisperContextParameters::default(),
        )
        .unwrap();

        let mut state = ctx.create_state().unwrap();
        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_translate(false); // Do NOT translate to English
        params.set_language(None); // Auto-detect language
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state.full(params, &audio_samples).unwrap();

        let n = state.full_n_segments();
        let mut text = String::new();
        for i in 0..n {
            if let Some(seg) = state.get_segment(i) {
                text.push_str(&seg.to_str_lossy().unwrap_or_default());
                text.push(' ');
            }
        }

        let lang_id = state.full_lang_id_from_state();
        (text, lang_id)
    })
    .await
    .unwrap();

    println!("=== Multilingual Whisper Auto-Detected Output ===");
    println!("{}", text.trim());
    println!("Detected lang ID: {}", detected_lang);
}
