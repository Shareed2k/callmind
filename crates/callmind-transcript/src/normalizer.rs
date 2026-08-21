use crate::vocabulary::{VocabularyEntry, VocabularyProcessor};

/// Text normalizer for multi-lingual Israeli call-center transcripts.
pub struct TextNormalizer;

impl TextNormalizer {
    /// Normalize raw STT text (currencies, numbers, vocabulary, symbols).
    pub fn normalize(raw_text: &str, vocabulary: &[VocabularyEntry]) -> String {
        if raw_text.trim().is_empty() {
            return String::new();
        }

        let mut text = raw_text.to_string();

        // 1. Apply Organization Vocabulary
        text = VocabularyProcessor::apply(&text, vocabulary);

        // 2. Normalize Israeli Currencies
        text = normalize_currencies(&text);

        // 3. Normalize Phone Numbers and Digit formatting
        text = normalize_phone_numbers(&text);

        // 4. Clean up whitespace
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// Normalize currencies across Hebrew, Russian, and English.
fn normalize_currencies(text: &str) -> String {
    let mut s = text.to_string();

    // Hebrew Shekel replacements
    s = s
        .replace(" שקלים", " ₪")
        .replace(" שקל", " ₪")
        .replace(" ש\"ח", " ₪")
        .replace(" ש״ח", " ₪")
        .replace(" NIS", " ₪")
        .replace(" nis", " ₪");

    // Dollars
    s = s
        .replace(" דולר", " $")
        .replace(" דולרים", " $")
        .replace(" dollars", " $")
        .replace(" dollar", " $");

    // Russian Rubles
    s = s
        .replace(" рублей", " ₽")
        .replace(" рубля", " ₽")
        .replace(" руб.", " ₽")
        .replace(" руб", " ₽");

    s
}

/// Format 10-digit Israeli mobile numbers (e.g., 0501234567 -> 050-123-4567).
fn normalize_phone_numbers(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut normalized_words = Vec::with_capacity(words.len());

    for word in words {
        if word.len() == 10 && word.starts_with("05") && word.chars().all(|c| c.is_ascii_digit()) {
            let formatted = format!("{}-{}-{}", &word[0..3], &word[3..6], &word[6..10]);
            normalized_words.push(formatted);
        } else {
            normalized_words.push(word.to_string());
        }
    }

    normalized_words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_normalization() {
        let raw = "החיוב שלך הוא 189 שקלים והמספר הוא 0501234567";
        let normalized = TextNormalizer::normalize(raw, &[]);
        assert_eq!(normalized, "החיוב שלך הוא 189 ₪ והמספר הוא 050-123-4567");

        let raw_ru = "Сумма заказа 500 рублей";
        let normalized_ru = TextNormalizer::normalize(raw_ru, &[]);
        assert_eq!(normalized_ru, "Сумма заказа 500 ₽");
    }
}
