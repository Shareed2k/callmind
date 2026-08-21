use callmind_audio::AudioBuffer;
use callmind_core::Language;
use callmind_language::{LanguageDetection, LanguageProbability};
use callmind_stt::{MockSttEngine, SttProfile, SttRouter, SttWord};
use std::sync::Arc;

#[tokio::test]
async fn test_stt_router_transcribe_end_to_end() {
    let hebrew_words = vec![
        SttWord::new("שלום".into(), 0, 500, Some(0.98), Some(Language::Hebrew)),
        SttWord::new("במה".into(), 550, 900, Some(0.95), Some(Language::Hebrew)),
        SttWord::new("אפשר".into(), 950, 1400, Some(0.99), Some(Language::Hebrew)),
        SttWord::new(
            "לעזור".into(),
            1450,
            1900,
            Some(0.97),
            Some(Language::Hebrew),
        ),
    ];

    let multi_words = vec![
        SttWord::new(
            "Здравствуйте".into(),
            0,
            800,
            Some(0.95),
            Some(Language::Russian),
        ),
        SttWord::new(
            "спасибо".into(),
            850,
            1400,
            Some(0.92),
            Some(Language::Russian),
        ),
    ];

    let hebrew_engine = Arc::new(MockSttEngine::new("ivrit-ai-v3", "1.0").with_words(hebrew_words));
    let multi_engine =
        Arc::new(MockSttEngine::new("whisper-large-v3", "1.0").with_words(multi_words));
    let router = SttRouter::new(hebrew_engine, multi_engine, 0.90);

    let audio = AudioBuffer::new(16000, 1, vec![0.05; 32000]); // 2 sec audio

    // Test 1: Hebrew call
    let hebrew_detection = LanguageDetection::new(
        Language::Hebrew,
        vec![LanguageProbability {
            language: Language::Hebrew,
            probability: 0.96,
        }],
        false,
    );

    let (stt_res, profile) = router
        .transcribe_routed(&audio, &hebrew_detection, &[])
        .await
        .unwrap();
    assert_eq!(profile, SttProfile::Hebrew);
    assert_eq!(stt_res.words.len(), 4);
    assert_eq!(stt_res.words[0].text, "שלום");
    assert_eq!(stt_res.words[0].start_ms, 0);
    assert_eq!(stt_res.words[0].end_ms, 500);

    // Test 2: Multilingual / Russian call
    let russian_detection = LanguageDetection::new(
        Language::Russian,
        vec![LanguageProbability {
            language: Language::Russian,
            probability: 0.92,
        }],
        false,
    );

    let (stt_res2, profile2) = router
        .transcribe_routed(&audio, &russian_detection, &[])
        .await
        .unwrap();
    assert_eq!(profile2, SttProfile::Multilingual);
    assert_eq!(stt_res2.words.len(), 2);
    assert_eq!(stt_res2.words[0].text, "Здравствуйте");
}
