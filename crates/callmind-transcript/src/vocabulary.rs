use callmind_core::Language;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Organization custom vocabulary term or alias.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VocabularyEntry {
    pub id: Uuid,
    pub phrase: String,
    pub aliases: Vec<String>,
    pub language: Option<Language>,
    pub priority: i32,
}

impl VocabularyEntry {
    pub fn new(phrase: String, aliases: Vec<String>, language: Option<Language>) -> Self {
        Self {
            id: Uuid::new_v4(),
            phrase,
            aliases,
            language,
            priority: 0,
        }
    }
}

/// Processor applying organization vocabulary aliases to text.
pub struct VocabularyProcessor;

impl VocabularyProcessor {
    /// Normalize text using custom vocabulary aliases.
    pub fn apply(text: &str, entries: &[VocabularyEntry]) -> String {
        if entries.is_empty() || text.is_empty() {
            return text.to_string();
        }

        let mut processed = text.to_string();

        for entry in entries {
            for alias in &entry.aliases {
                if !alias.is_empty() && processed.contains(alias) {
                    processed = processed.replace(alias, &entry.phrase);
                }
            }
        }

        processed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vocabulary_replacement() {
        let entries = vec![
            VocabularyEntry::new(
                "Wolt".into(),
                vec!["וולט".into(), "volt".into()],
                Some(Language::Hebrew),
            ),
            VocabularyEntry::new(
                "חשבונית מס".into(),
                vec!["חשבונית".into()],
                Some(Language::Hebrew),
            ),
        ];

        let raw = "הזמנתי דרך וולט ואני צריך חשבונית";
        let normalized = VocabularyProcessor::apply(raw, &entries);
        assert_eq!(normalized, "הזמנתי דרך Wolt ואני צריך חשבונית מס");
    }
}
