use crate::errors::SttError;
use crate::models::{SttRequest, SttResult};
use crate::traits::SttEngine;
use callmind_audio::AudioBuffer;
use callmind_core::Language;
use callmind_language::LanguageDetection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// STT Model Profile selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SttProfile {
    /// Fine-tuned Hebrew STT Model (e.g. ivrit-ai Whisper Large v3)
    Hebrew,
    /// General Multilingual Whisper Model (e.g. Whisper Large v3 Multilingual for Russian/English/Mixed)
    Multilingual,
}

/// Dynamic STT model router that directs audio to specialized models based on language scan.
#[derive(Clone)]
pub struct SttRouter {
    pub hebrew_engine: Arc<dyn SttEngine>,
    pub multilingual_engine: Arc<dyn SttEngine>,
    pub hebrew_threshold: f32,
}

impl SttRouter {
    pub fn new(
        hebrew_engine: Arc<dyn SttEngine>,
        multilingual_engine: Arc<dyn SttEngine>,
        hebrew_threshold: f32,
    ) -> Self {
        Self {
            hebrew_engine,
            multilingual_engine,
            hebrew_threshold,
        }
    }

    /// Select the appropriate STT profile based on language scan probabilities.
    pub fn choose_profile(&self, detection: &LanguageDetection) -> SttProfile {
        if detection.primary == Language::Hebrew
            && !detection.mixed
            && detection.hebrew_ratio() >= self.hebrew_threshold
        {
            SttProfile::Hebrew
        } else {
            SttProfile::Multilingual
        }
    }

    /// Retrieve the corresponding STT engine instance.
    pub fn select_engine(&self, profile: SttProfile) -> Arc<dyn SttEngine> {
        match profile {
            SttProfile::Hebrew => self.hebrew_engine.clone(),
            SttProfile::Multilingual => self.multilingual_engine.clone(),
        }
    }

    /// Execute routed transcription based on detected language distribution.
    pub async fn transcribe_routed(
        &self,
        audio: &AudioBuffer,
        detection: &LanguageDetection,
        vocabulary: &[String],
    ) -> Result<(SttResult, SttProfile), SttError> {
        let profile = self.choose_profile(detection);

        // 1. Explicit Hebrew detection satisfying threshold -> route directly to fine-tuned ivrit-ai
        if profile == SttProfile::Hebrew {
            let req = SttRequest {
                audio,
                language_hint: Some(Language::Hebrew),
                vocabulary,
                word_timestamps: true,
            };
            let result = self.hebrew_engine.transcribe(req).await?;
            return Ok((result, SttProfile::Hebrew));
        }

        // 2. Use full-audio auto-detection for every non-Hebrew call. The short
        // language probe is only reliable enough to select the specialized
        // Hebrew model; it must not force a potentially wrong language hint.
        let req = SttRequest {
            audio,
            language_hint: None,
            vocabulary,
            word_timestamps: true,
        };

        let result = self.multilingual_engine.transcribe(req).await?;

        // If Whisper auto-detected Hebrew as the dominant language, switch to ivrit-ai for superior Hebrew transcription
        if let Some(ref detected) = result.detected_language {
            if *detected == Language::Hebrew {
                let hebrew_req = SttRequest {
                    audio,
                    language_hint: Some(Language::Hebrew),
                    vocabulary,
                    word_timestamps: true,
                };
                if let Ok(hebrew_res) = self.hebrew_engine.transcribe(hebrew_req).await {
                    return Ok((hebrew_res, SttProfile::Hebrew));
                }
            }
        }

        Ok((result, SttProfile::Multilingual))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockSttEngine;
    use callmind_language::LanguageProbability;

    #[test]
    fn test_stt_router_hebrew_vs_multilingual() {
        let hebrew_engine = Arc::new(MockSttEngine::new("ivrit-ai-v3", "1.0"));
        let multi_engine = Arc::new(MockSttEngine::new("whisper-large-v3", "1.0"));
        let router = SttRouter::new(hebrew_engine, multi_engine, 0.90);

        // 1. Pure Hebrew (>90%) -> Hebrew profile
        let pure_hebrew = LanguageDetection::new(
            Language::Hebrew,
            vec![LanguageProbability {
                language: Language::Hebrew,
                probability: 0.95,
            }],
            false,
        );
        assert_eq!(router.choose_profile(&pure_hebrew), SttProfile::Hebrew);

        // 2. Mixed Hebrew / Russian -> Multilingual profile
        let mixed_he_ru = LanguageDetection::new(
            Language::Hebrew,
            vec![
                LanguageProbability {
                    language: Language::Hebrew,
                    probability: 0.70,
                },
                LanguageProbability {
                    language: Language::Russian,
                    probability: 0.30,
                },
            ],
            true,
        );
        assert_eq!(
            router.choose_profile(&mixed_he_ru),
            SttProfile::Multilingual
        );

        // 3. Russian call -> Multilingual profile
        let russian = LanguageDetection::new(
            Language::Russian,
            vec![LanguageProbability {
                language: Language::Russian,
                probability: 0.98,
            }],
            false,
        );
        assert_eq!(router.choose_profile(&russian), SttProfile::Multilingual);
    }
}
