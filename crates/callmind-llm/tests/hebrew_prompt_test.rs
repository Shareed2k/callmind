use callmind_core::Language;
use callmind_llm::prompts::{build_analysis_prompt, build_language_aware_analysis_prompt};

#[test]
fn test_hebrew_language_aware_prompt() {
    let transcript = "[0] (00:01) דובר 0: שלום, אני רוצה לקבוע תור למחר בבוקר";
    let prompt = build_language_aware_analysis_prompt(transcript, "Personal", &Language::Hebrew);

    assert!(prompt.contains("עברית"));
    assert!(prompt.contains("JSON"));
    assert!(prompt.contains("title"));
    assert!(prompt.contains("summary"));
}

#[test]
fn test_default_prompt_backward_compatibility() {
    let transcript = "Hello, how are you?";
    let prompt = build_analysis_prompt(transcript, "Personal");
    assert!(prompt.contains("title"));
    assert!(prompt.contains("summary"));
}
