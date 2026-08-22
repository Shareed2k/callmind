use serde_json::Value;
use std::fmt::Write;

/// Unified Rich Response for Bot Channels (Telegram, WhatsApp, Slack, Webhook).
#[derive(Debug, Clone)]
pub struct FormattedBotResponse {
    pub title: String,
    pub summary: String,
    pub text_markdown: String,
    pub has_calendar_event: bool,
    pub ics_content: Option<String>,
    pub web_player_url: String,
}

/// Formatter creating beautiful, structured messages across Hebrew, Russian, and English for any bot channel.
pub struct BotResponseFormatter;

impl BotResponseFormatter {
    /// Format analysis JSON into a clean, human-readable bot message with To-Dos, key facts, and calendar.
    pub fn format(
        call_id: &str,
        full_analysis_json: &str,
        server_bind: &str,
    ) -> FormattedBotResponse {
        let parsed: Value = serde_json::from_str(full_analysis_json).unwrap_or_default();

        let title = parsed["title"]
            .as_str()
            .unwrap_or("CallMind Conversation")
            .to_string();

        let summary = parsed["summary"]
            .as_str()
            .unwrap_or("Conversation processed successfully.")
            .to_string();

        let mut msg = format!("📌 *{title}*\n\n📋 *Summary:*\n{summary}\n");

        // Action Items & To-Dos
        if let Some(actions) = parsed["action_items"].as_array() {
            if !actions.is_empty() {
                msg.push_str("\n📝 *Action Items & To-Dos:*\n");
                for item in actions {
                    let text = item["text"].as_str().unwrap_or("");
                    if !text.is_empty() {
                        let owner = item["owner"]
                            .as_str()
                            .map_or_else(String::new, |o| format!(" [{o}]"));
                        let deadline = item["deadline"]
                            .as_str()
                            .map_or_else(String::new, |d| format!(" (⏰ {d})"));
                        let _ = writeln!(msg, "• [ ] {text}{owner}{deadline}");
                    }
                }
            }
        }

        // Key Facts & Details
        if let Some(facts) = parsed["key_facts"].as_array() {
            if !facts.is_empty() {
                msg.push_str("\n📍 *Key Facts & Details:*\n");
                for fact in facts {
                    if let Some(f_str) = fact.as_str() {
                        let _ = writeln!(msg, "• {f_str}");
                    }
                }
            }
        }

        // Extracted Entities (Addresses, Phones, Amounts)
        if let Some(entities) = parsed["entities"].as_array() {
            let notable: Vec<String> = entities
                .iter()
                .filter_map(|e| {
                    let val = e["value"].as_str()?;
                    let t = e["entity_type"].as_str().unwrap_or("");
                    if t.contains("loc")
                        || t.contains("address")
                        || t.contains("phone")
                        || t.contains("price")
                        || t.contains("amount")
                    {
                        Some(format!("• {val} ({t})"))
                    } else {
                        None
                    }
                })
                .collect();

            if !notable.is_empty() {
                msg.push_str("\n🏷️ *Extracted Entities:*\n");
                for note in notable {
                    let _ = writeln!(msg, "{note}");
                }
            }
        }

        let web_player_url = format!("http://{server_bind}/calls/{call_id}");
        let _ = write!(msg, "\n🔗 [Open in CallMind Web Player]({web_player_url})");

        // Detect if location or appointment exists to generate .ics Calendar
        let location = parsed["entities"].as_array().and_then(|arr| {
            arr.iter().find_map(|item| {
                let t = item.get("entity_type")?.as_str()?;
                if t.contains("loc") || t.contains("address") || t.contains("place") {
                    item.get("value")?.as_str()
                } else {
                    None
                }
            })
        });

        let has_calendar_event = location.is_some()
            || parsed["reason"].as_str().is_some_and(|r| {
                r.contains("встреч")
                    || r.contains("прием")
                    || r.contains("appointment")
                    || r.contains("תור")
            });

        let ics_content = if has_calendar_event {
            Some(callmind_transcript::TranscriptExporter::to_ics(
                call_id, &title, &summary, location, None,
            ))
        } else {
            None
        };

        FormattedBotResponse {
            title,
            summary,
            text_markdown: msg,
            has_calendar_event,
            ics_content,
            web_player_url,
        }
    }
}
