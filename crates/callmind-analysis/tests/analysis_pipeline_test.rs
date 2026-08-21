use callmind_analysis::{AnalysisEngine, Classifier};
use callmind_core::{CallId, Language, SpeakerId, SpeakerRole};
use callmind_llm::MockLlmEngine;
use callmind_transcript::{TextDirection, Transcript, TranscriptSegment, TranscriptWord};
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn test_conversation_analysis_pipeline() {
    let call_id = CallId::generate();

    let seg1 = TranscriptSegment {
        id: Uuid::new_v4(),
        call_id,
        sequence: 0,
        speaker_id: SpeakerId::new(0),
        speaker_role: SpeakerRole::Agent,
        language: Language::Hebrew,
        text_direction: TextDirection::Rtl,
        start_ms: 0,
        end_ms: 3000,
        raw_text: "שלום, מדבר דני מחברת בזק, במה אפשר לעזור?".into(),
        normalized_text: "שלום, מדבר דני מחברת בזק, במה אפשר לעזור?".into(),
        words: vec![TranscriptWord {
            text: "שלום".into(),
            start_ms: 0,
            end_ms: 500,
            speaker_id: SpeakerId::new(0),
            speaker_role: SpeakerRole::Agent,
            language: Language::Hebrew,
            confidence: None,
        }],
    };

    let seg2 = TranscriptSegment {
        id: Uuid::new_v4(),
        call_id,
        sequence: 1,
        speaker_id: SpeakerId::new(1),
        speaker_role: SpeakerRole::Customer,
        language: Language::Russian,
        text_direction: TextDirection::Ltr,
        start_ms: 3500,
        end_ms: 7000,
        raw_text: "Здравствуйте, я хочу отменить подписку".into(),
        normalized_text: "Здравствуйте, я хочу отменить подписку".into(),
        words: vec![TranscriptWord {
            text: "Здравствуйте".into(),
            start_ms: 3500,
            end_ms: 4500,
            speaker_id: SpeakerId::new(1),
            speaker_role: SpeakerRole::Customer,
            language: Language::Russian,
            confidence: None,
        }],
    };

    let transcript = Transcript {
        call_id,
        languages: Vec::new(),
        speakers: Vec::new(),
        segments: vec![seg1, seg2],
    };

    let mock_json = serde_json::json!({
        "title": "Subscription Cancellation Request",
        "summary": "Customer called requesting subscription cancellation. Agent acknowledged request.",
        "reason": "Cancellation of internet service",
        "resolution": "Cancellation request processed",
        "resolved": true,
        "customer_intent": "cancellation",
        "topics": ["cancellation", "subscription", "support"],
        "action_items": [
            {
                "text": "Send confirmation email to customer",
                "owner": "agent",
                "deadline": "within 24 hours",
                "evidence_segments": [0, 1]
            }
        ],
        "entities": [
            {
                "entity_type": "company",
                "value": "Bezeq",
                "evidence_segments": [0]
            }
        ],
        "sentiment_score": -0.4,
        "scorecard": {
            "total_score": 88,
            "rules": [
                {
                    "name": "Greeting & Identity",
                    "score": 20,
                    "max_score": 20,
                    "explanation": "Agent properly introduced company name and greeted customer",
                    "evidence_segments": [0]
                }
            ]
        },
        "compliance": [
            {
                "name": "Recording Disclosure",
                "passed": true,
                "explanation": "Call was disclosed",
                "evidence_segments": [0]
            }
        ]
    });

    let mock_llm = Arc::new(MockLlmEngine::new().with_json(mock_json));
    let analysis_engine = AnalysisEngine::new(mock_llm);

    let classifiers = vec![Classifier::new_boolean(
        "Cancellation Request",
        "Customer explicitly asked to cancel",
    )];

    let analysis = analysis_engine
        .analyze(&transcript, "Bezeq", &classifiers)
        .await
        .unwrap();

    assert_eq!(analysis.title, "Subscription Cancellation Request");
    assert!(analysis.resolved);
    assert_eq!(analysis.customer_intent.as_deref(), Some("cancellation"));
    assert_eq!(analysis.action_items.len(), 1);
    assert_eq!(analysis.action_items[0].owner, Some(SpeakerRole::Speaker1));
    assert_eq!(
        analysis.action_items[0].evidence.segment_indices,
        vec![0, 1]
    );

    assert!(analysis.scorecard.is_some());
    assert_eq!(analysis.scorecard.unwrap().total_score, 88);

    assert_eq!(analysis.compliance.len(), 1);
    assert!(analysis.compliance[0].passed);

    assert_eq!(analysis.classifiers.len(), 1);
    assert_eq!(analysis.classifiers[0].value, serde_json::Value::Bool(true));
}
