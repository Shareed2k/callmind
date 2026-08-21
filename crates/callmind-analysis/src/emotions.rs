use callmind_core::Language;
use serde::{Deserialize, Serialize};

/// The 8 Plutchik emotional states + Neutral (aligned with Hebrew hebEMO & heBERT).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EmotionType {
    Anger,
    Fear,
    Joy,
    Surprise,
    Sadness,
    Disgust,
    Anticipation,
    Trust,
    Neutral,
}

impl std::fmt::Display for EmotionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anger => write!(f, "Anger (כעס / Гнев)"),
            Self::Fear => write!(f, "Fear (פחד / Страх)"),
            Self::Joy => write!(f, "Joy (שמחה / Радость)"),
            Self::Surprise => write!(f, "Surprise (הפתעה / Удивление)"),
            Self::Sadness => write!(f, "Sadness (עצב / Грусть)"),
            Self::Disgust => write!(f, "Disgust (סלידה / Отвращение)"),
            Self::Anticipation => write!(f, "Anticipation (ציפייה / Ожидание)"),
            Self::Trust => write!(f, "Trust (אמון / Доверие)"),
            Self::Neutral => write!(f, "Neutral (ניטרלי / Нейтрально)"),
        }
    }
}

/// Distribution of emotional intensities in the analyzed text (normalized from 0.0 to 1.0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EmotionDistribution {
    pub anger: f32,
    pub fear: f32,
    pub joy: f32,
    pub surprise: f32,
    pub sadness: f32,
    pub disgust: f32,
    pub anticipation: f32,
    pub trust: f32,
    pub neutral: f32,
    pub dominant_emotion: EmotionType,
}

impl Default for EmotionDistribution {
    fn default() -> Self {
        Self {
            anger: 0.0,
            fear: 0.0,
            joy: 0.0,
            surprise: 0.0,
            sadness: 0.0,
            disgust: 0.0,
            anticipation: 0.0,
            trust: 0.0,
            neutral: 1.0,
            dominant_emotion: EmotionType::Neutral,
        }
    }
}

/// Hebrew, Russian, and English multilingual emotion analyzer.
pub struct EmotionClassifier;

impl EmotionClassifier {
    #[must_use]
    pub fn analyze_text(text: &str, language: &Language) -> EmotionDistribution {
        let lower = text.to_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();

        if words.is_empty() {
            return EmotionDistribution::default();
        }

        let mut counts = [0.0f32; 8]; // anger, fear, joy, surprise, sadness, disgust, anticipation, trust

        match language {
            Language::Hebrew => {
                Self::score_hebrew(&lower, &mut counts);
            }
            Language::Russian => {
                Self::score_russian(&lower, &mut counts);
            }
            _ => {
                Self::score_english(&lower, &mut counts);
                // Also check across other languages if multilingual
                Self::score_hebrew(&lower, &mut counts);
                Self::score_russian(&lower, &mut counts);
            }
        }

        let total_hits: f32 = counts.iter().sum();
        if total_hits < 0.01 {
            return EmotionDistribution {
                anger: 0.0,
                fear: 0.0,
                joy: 0.0,
                surprise: 0.0,
                sadness: 0.0,
                disgust: 0.0,
                anticipation: 0.0,
                trust: 0.0,
                neutral: 1.0,
                dominant_emotion: EmotionType::Neutral,
            };
        }

        let anger = counts[0] / total_hits;
        let fear = counts[1] / total_hits;
        let joy = counts[2] / total_hits;
        let surprise = counts[3] / total_hits;
        let sadness = counts[4] / total_hits;
        let disgust = counts[5] / total_hits;
        let anticipation = counts[6] / total_hits;
        let trust = counts[7] / total_hits;

        let mut max_val = 0.0f32;
        let mut dominant = EmotionType::Neutral;

        let pairs = [
            (anger, EmotionType::Anger),
            (fear, EmotionType::Fear),
            (joy, EmotionType::Joy),
            (surprise, EmotionType::Surprise),
            (sadness, EmotionType::Sadness),
            (disgust, EmotionType::Disgust),
            (anticipation, EmotionType::Anticipation),
            (trust, EmotionType::Trust),
        ];

        for (val, emo) in pairs {
            if val > max_val {
                max_val = val;
                dominant = emo;
            }
        }

        EmotionDistribution {
            anger,
            fear,
            joy,
            surprise,
            sadness,
            disgust,
            anticipation,
            trust,
            neutral: (1.0 - (total_hits / 5.0).min(1.0)).max(0.0),
            dominant_emotion: dominant,
        }
    }

    fn score_hebrew(text: &str, counts: &mut [f32; 8]) {
        // 0: Anger (כעס)
        let anger_words = [
            "כועס",
            "זעם",
            "עצבני",
            "בושה",
            "תפסיק",
            "נורא",
            "תלונה",
            "מרגיז",
            "שקר",
            "עצבים",
        ];
        for w in anger_words {
            if text.contains(w) {
                counts[0] += 1.0;
            }
        }

        // 1: Fear (פחד)
        let fear_words = [
            "מפחד",
            "פחד",
            "חרדה",
            "סכנה",
            "דאגה",
            "מלחיץ",
            "בהלה",
            "להילחץ",
            "נלחץ",
        ];
        for w in fear_words {
            if text.contains(w) {
                counts[1] += 1.0;
            }
        }

        // 2: Joy (שמחה)
        let joy_words = [
            "שמח",
            "מעולה",
            "כיף",
            "נהדר",
            "מצוין",
            "תודה",
            "ברכות",
            "יופי",
            "תענוג",
            "מעריכים",
        ];
        for w in joy_words {
            if text.contains(w) {
                counts[2] += 1.0;
            }
        }

        // 3: Surprise (הפתעה)
        let surprise_words = [
            "מופתע",
            "באמת",
            "לא ייאמן",
            "ואו",
            "וואו",
            "מה פתאום",
            "שוק",
            "פתאום",
        ];
        for w in surprise_words {
            if text.contains(w) {
                counts[3] += 1.0;
            }
        }

        // 4: Sadness (עצב)
        let sadness_words = [
            "עצוב",
            "חבל",
            "מאכזב",
            "מסכן",
            "כואב",
            "צער",
            "בדידות",
            "אכזבה",
        ];
        for w in sadness_words {
            if text.contains(w) {
                counts[4] += 1.0;
            }
        }

        // 5: Disgust (סלידה)
        let disgust_words = ["מגעיל", "דוחה", "גרוע", "מזעזע", "איכס", "מחריד", "גועל"];
        for w in disgust_words {
            if text.contains(w) {
                counts[5] += 1.0;
            }
        }

        // 6: Anticipation (ציפייה)
        let anticipation_words = [
            "מחכה",
            "מקווה",
            "מתי",
            "בקרוב",
            "מצפה",
            "תעדכן",
            "ממתין",
            "נמתין",
        ];
        for w in anticipation_words {
            if text.contains(w) {
                counts[6] += 1.0;
            }
        }

        // 7: Trust (אמון)
        let trust_words = [
            "מסכים",
            "בטח",
            "סומך",
            "בסדר גמור",
            "אין בעיה",
            "כמובן",
            "מאמין",
            "מקובל",
            "הבנתי",
        ];
        for w in trust_words {
            if text.contains(w) {
                counts[7] += 1.0;
            }
        }
    }

    fn score_russian(text: &str, counts: &mut [f32; 8]) {
        // 0: Anger (Гнев)
        let anger_words = [
            "злой",
            "злость",
            "возмутительно",
            "кошмар",
            "безобразие",
            "ненавижу",
            "крик",
            "обман",
            "бесит",
        ];
        for w in anger_words {
            if text.contains(w) {
                counts[0] += 1.0;
            }
        }

        // 1: Fear (Страх)
        let fear_words = [
            "страх",
            "боюсь",
            "тревога",
            "паника",
            "опасно",
            "испуг",
            "страшно",
            "переживаю",
        ];
        for w in fear_words {
            if text.contains(w) {
                counts[1] += 1.0;
            }
        }

        // 2: Joy (Радость)
        let joy_words = [
            "спасибо",
            "отлично",
            "замечательно",
            "супер",
            "рад",
            "прекрасно",
            "молодец",
            "здорово",
            "благодарю",
        ];
        for w in joy_words {
            if text.contains(w) {
                counts[2] += 1.0;
            }
        }

        // 3: Surprise (Удивление)
        let surprise_words = [
            "неужели",
            "серьезно",
            "вау",
            "ничего себе",
            "шок",
            "неожиданно",
        ];
        for w in surprise_words {
            if text.contains(w) {
                counts[3] += 1.0;
            }
        }

        // 4: Sadness (Грусть)
        let sadness_words = [
            "грустно",
            "жаль",
            "обидно",
            "печально",
            "разочарован",
            "тоска",
            "сочувствую",
        ];
        for w in sadness_words {
            if text.contains(w) {
                counts[4] += 1.0;
            }
        }

        // 5: Disgust (Отвращение)
        let disgust_words = [
            "отвратительно",
            "мерзко",
            "ужасно",
            "фу",
            "гадость",
            "паршиво",
        ];
        for w in disgust_words {
            if text.contains(w) {
                counts[5] += 1.0;
            }
        }

        // 6: Anticipation (Ожидание)
        let anticipation_words = ["жду", "надеюсь", "когда", "скоро", "ожидаю", "рассчитываю"];
        for w in anticipation_words {
            if text.contains(w) {
                counts[6] += 1.0;
            }
        }

        // 7: Trust (Доверие)
        let trust_words = [
            "согласен",
            "доверяю",
            "конечно",
            "без проблем",
            "договорились",
            "верю",
            "понял",
        ];
        for w in trust_words {
            if text.contains(w) {
                counts[7] += 1.0;
            }
        }
    }

    fn score_english(text: &str, counts: &mut [f32; 8]) {
        // 0: Anger
        let anger_words = [
            "angry",
            "furious",
            "outrageous",
            "unacceptable",
            "annoying",
            "hate",
            "mad",
        ];
        for w in anger_words {
            if text.contains(w) {
                counts[0] += 1.0;
            }
        }

        // 1: Fear
        let fear_words = ["afraid", "scared", "anxiety", "worried", "panic", "danger"];
        for w in fear_words {
            if text.contains(w) {
                counts[1] += 1.0;
            }
        }

        // 2: Joy
        let joy_words = [
            "thank",
            "thanks",
            "great",
            "excellent",
            "wonderful",
            "happy",
            "amazing",
            "glad",
            "awesome",
        ];
        for w in joy_words {
            if text.contains(w) {
                counts[2] += 1.0;
            }
        }

        // 3: Surprise
        let surprise_words = ["wow", "really", "unbelievable", "shocked", "unexpected"];
        for w in surprise_words {
            if text.contains(w) {
                counts[3] += 1.0;
            }
        }

        // 4: Sadness
        let sadness_words = ["sad", "sorry", "disappointed", "unfortunate", "regret"];
        for w in sadness_words {
            if text.contains(w) {
                counts[4] += 1.0;
            }
        }

        // 5: Disgust
        let disgust_words = ["disgusting", "awful", "terrible", "gross", "nasty"];
        for w in disgust_words {
            if text.contains(w) {
                counts[5] += 1.0;
            }
        }

        // 6: Anticipation
        let anticipation_words = ["waiting", "hoping", "expecting", "looking forward", "soon"];
        for w in anticipation_words {
            if text.contains(w) {
                counts[6] += 1.0;
            }
        }

        // 7: Trust
        let trust_words = [
            "agree",
            "trust",
            "definitely",
            "absolutely",
            "confident",
            "sure",
            "understood",
        ];
        for w in trust_words {
            if text.contains(w) {
                counts[7] += 1.0;
            }
        }
    }
}
