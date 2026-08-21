use callmind_core::Language;
use serde::{Deserialize, Serialize};

/// Estimated probability for an individual language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LanguageProbability {
    pub language: Language,
    pub probability: f32,
}

/// Comprehensive language detection result for an entire conversation or speech sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LanguageDetection {
    /// Dominant primary language detected.
    pub primary: Language,
    /// Probability distribution across detected languages.
    pub distribution: Vec<LanguageProbability>,
    /// Flag indicating whether code-switching / multiple languages are present.
    pub mixed: bool,
}

impl LanguageDetection {
    pub fn new(primary: Language, distribution: Vec<LanguageProbability>, mixed: bool) -> Self {
        Self {
            primary,
            distribution,
            mixed,
        }
    }

    /// Return probability/ratio for a specific language (0.0 to 1.0).
    pub fn get_ratio(&self, language: &Language) -> f32 {
        self.distribution
            .iter()
            .find(|lp| &lp.language == language)
            .map_or(0.0, |lp| lp.probability)
    }

    /// Helper returning the Hebrew ratio.
    pub fn hebrew_ratio(&self) -> f32 {
        self.get_ratio(&Language::Hebrew)
    }

    /// Helper returning the Russian ratio.
    pub fn russian_ratio(&self) -> f32 {
        self.get_ratio(&Language::Russian)
    }

    /// Helper returning the English ratio.
    pub fn english_ratio(&self) -> f32 {
        self.get_ratio(&Language::English)
    }

    /// Helper returning the confidence score of the primary language.
    pub fn confidence(&self) -> f32 {
        self.get_ratio(&self.primary)
    }
}
