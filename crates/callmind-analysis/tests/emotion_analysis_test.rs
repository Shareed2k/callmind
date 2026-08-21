use callmind_analysis::emotions::{EmotionClassifier, EmotionType};
use callmind_core::Language;

#[test]
fn test_hebrew_emotion_detection() {
    let text = "תודה רבה לך, זה פשוט מצוין ומעולה! שמחתי מאוד לדבר איתך";
    let dist = EmotionClassifier::analyze_text(text, &Language::Hebrew);
    assert_eq!(dist.dominant_emotion, EmotionType::Joy);
    assert!(dist.joy > 0.0);

    let angry_text = "תפסיק לדבר שטויות! השירות שלכם פשוט גרוע ומזעזע, אני כועס מאוד";
    let angry_dist = EmotionClassifier::analyze_text(angry_text, &Language::Hebrew);
    assert!(angry_dist.anger > 0.0 || angry_dist.disgust > 0.0);
}

#[test]
fn test_russian_emotion_detection() {
    let text = "Огромное спасибо за помощь, все просто отлично и замечательно!";
    let dist = EmotionClassifier::analyze_text(text, &Language::Russian);
    assert_eq!(dist.dominant_emotion, EmotionType::Joy);
    assert!(dist.joy > 0.0);

    let angry_text = "Это возмутительно и ужасно, кошмарный сервис!";
    let angry_dist = EmotionClassifier::analyze_text(angry_text, &Language::Russian);
    assert!(angry_dist.anger > 0.0 || angry_dist.disgust > 0.0);
}

#[test]
fn test_english_emotion_detection() {
    let text = "Thank you so much, this is great and wonderful!";
    let dist = EmotionClassifier::analyze_text(text, &Language::English);
    assert_eq!(dist.dominant_emotion, EmotionType::Joy);
    assert!(dist.joy > 0.0);
}
