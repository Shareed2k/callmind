use crate::models::TextDirection;
use callmind_core::Language;

/// Helper for determining text rendering directionality (RTL vs LTR) and language.
pub struct RtlDetector;

impl RtlDetector {
    /// Detect text direction based on language and character unicode analysis.
    #[must_use]
    pub fn detect(text: &str, language: &Language) -> TextDirection {
        match language {
            Language::Hebrew | Language::Arabic => TextDirection::Rtl,
            Language::Russian | Language::English => TextDirection::Ltr,
            _ => {
                let mut rtl_chars = 0;
                let mut ltr_chars = 0;

                for ch in text.chars() {
                    if is_hebrew_or_arabic(ch) {
                        rtl_chars += 1;
                    } else if ch.is_alphabetic() {
                        ltr_chars += 1;
                    }
                }

                if rtl_chars > ltr_chars {
                    TextDirection::Rtl
                } else {
                    TextDirection::Ltr
                }
            }
        }
    }

    /// Detect language from character unicode distributions and n-gram analysis (Hebrew, Russian, English).
    #[must_use]
    pub fn detect_language(text: &str) -> Language {
        if let Some(info) = whatlang::detect(text) {
            match info.lang() {
                whatlang::Lang::Heb => return Language::Hebrew,
                whatlang::Lang::Rus => return Language::Russian,
                whatlang::Lang::Eng => return Language::English,
                whatlang::Lang::Ara => return Language::Arabic,
                _ => {}
            }
        }

        let mut hebrew = 0;
        let mut cyrillic = 0;
        let mut latin = 0;

        for ch in text.chars() {
            if is_hebrew_or_arabic(ch) {
                hebrew += 1;
            } else if matches!(ch, '\u{0400}'..='\u{04FF}' | '\u{0500}'..='\u{052F}') {
                cyrillic += 1;
            } else if ch.is_ascii_alphabetic() {
                latin += 1;
            }
        }

        if hebrew > cyrillic && hebrew > latin {
            Language::Hebrew
        } else if cyrillic > hebrew && cyrillic > latin {
            Language::Russian
        } else if latin > 0 {
            Language::English
        } else {
            Language::Unknown
        }
    }
}

/// Check if a character belongs to Hebrew or Arabic Unicode blocks.
fn is_hebrew_or_arabic(ch: char) -> bool {
    matches!(ch,
        '\u{0590}'..='\u{05FF}' | // Hebrew
        '\u{FB1D}'..='\u{FB4F}' | // Hebrew Presentation Forms
        '\u{0600}'..='\u{06FF}' | // Arabic
        '\u{0750}'..='\u{077F}' | // Arabic Supplement
        '\u{FB50}'..='\u{FDFF}' | // Arabic Presentation Forms-A
        '\u{FE70}'..='\u{FEFF}'   // Arabic Presentation Forms-B
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rtl_detection() {
        assert_eq!(
            RtlDetector::detect("שלום, במה אפשר לעזור?", &Language::Hebrew),
            TextDirection::Rtl
        );
        assert_eq!(
            RtlDetector::detect("Здравствуйте, у меня проблема", &Language::Russian),
            TextDirection::Ltr
        );
        assert_eq!(
            RtlDetector::detect("Hello how can I help you", &Language::English),
            TextDirection::Ltr
        );
        assert_eq!(
            RtlDetector::detect_language("שלום מה נשמע"),
            Language::Hebrew
        );
        assert_eq!(
            RtlDetector::detect_language("Здравствуйте как дела"),
            Language::Russian
        );
        assert_eq!(
            RtlDetector::detect_language("Hello how are you"),
            Language::English
        );
    }
}
