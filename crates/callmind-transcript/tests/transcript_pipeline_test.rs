use callmind_audio::ChannelMode;
use callmind_core::{CallId, Language, SpeakerId, SpeakerRole};
use callmind_language::LanguageProbability;
use callmind_stt::SttWord;
use callmind_transcript::{TextDirection, TranscriptBuilder, VocabularyEntry};

#[test]
fn test_transcript_builder_multilingual_and_rtl() {
    let call_id = CallId::generate();

    let words = vec![
        // Agent (Hebrew, RTL, Speaker 0)
        SttWord::new("שלום".into(), 0, 500, Some(0.98), Some(Language::Hebrew))
            .with_speaker(SpeakerId::new(0)),
        SttWord::new("מדבר".into(), 550, 900, Some(0.95), Some(Language::Hebrew))
            .with_speaker(SpeakerId::new(0)),
        SttWord::new("דני".into(), 950, 1300, Some(0.96), Some(Language::Hebrew))
            .with_speaker(SpeakerId::new(0)),
        SttWord::new("במה".into(), 1350, 1700, Some(0.94), Some(Language::Hebrew))
            .with_speaker(SpeakerId::new(0)),
        SttWord::new(
            "אפשר".into(),
            1750,
            2100,
            Some(0.99),
            Some(Language::Hebrew),
        )
        .with_speaker(SpeakerId::new(0)),
        SttWord::new(
            "לעזור".into(),
            2150,
            2600,
            Some(0.97),
            Some(Language::Hebrew),
        )
        .with_speaker(SpeakerId::new(0)),
        // Customer (Russian, LTR, Speaker 1) - after a pause > 1500ms
        SttWord::new(
            "Здравствуйте".into(),
            4500,
            5200,
            Some(0.95),
            Some(Language::Russian),
        )
        .with_speaker(SpeakerId::new(1)),
        SttWord::new("у".into(), 5250, 5400, Some(0.92), Some(Language::Russian))
            .with_speaker(SpeakerId::new(1)),
        SttWord::new(
            "меня".into(),
            5450,
            5800,
            Some(0.98),
            Some(Language::Russian),
        )
        .with_speaker(SpeakerId::new(1)),
        SttWord::new(
            "проблема".into(),
            5850,
            6400,
            Some(0.94),
            Some(Language::Russian),
        )
        .with_speaker(SpeakerId::new(1)),
        SttWord::new("с".into(), 6450, 6600, Some(0.91), Some(Language::Russian))
            .with_speaker(SpeakerId::new(1)),
        SttWord::new(
            "заказом".into(),
            6650,
            7200,
            Some(0.97),
            Some(Language::Russian),
        )
        .with_speaker(SpeakerId::new(1)),
    ];

    let languages = vec![
        LanguageProbability {
            language: Language::Hebrew,
            probability: 0.65,
        },
        LanguageProbability {
            language: Language::Russian,
            probability: 0.35,
        },
    ];

    let vocabulary = vec![VocabularyEntry::new(
        "דני".into(),
        vec!["דניאל".into()],
        Some(Language::Hebrew),
    )];

    let transcript =
        TranscriptBuilder::build(call_id, &words, &ChannelMode::Mono, &vocabulary, languages);

    assert_eq!(transcript.segments.len(), 2);

    // Segment 1: Agent (Hebrew, RTL)
    let seg1 = &transcript.segments[0];
    assert_eq!(seg1.speaker_role, SpeakerRole::Agent);
    assert_eq!(seg1.language, Language::Hebrew);
    assert_eq!(seg1.text_direction, TextDirection::Rtl);
    assert_eq!(seg1.raw_text, "שלום מדבר דני במה אפשר לעזור");

    // Segment 2: Customer (Russian, LTR)
    let seg2 = &transcript.segments[1];
    assert_eq!(seg2.speaker_role, SpeakerRole::Customer);
    assert_eq!(seg2.language, Language::Russian);
    assert_eq!(seg2.text_direction, TextDirection::Ltr);
    assert_eq!(seg2.raw_text, "Здравствуйте у меня проблема с заказом");

    // Full text formatted
    let full = transcript.full_text();
    assert!(full.contains("agent: שלום מדבר דני במה אפשר לעזור"));
    assert!(full.contains("customer: Здравствуйте у меня проблема с заказом"));
}
