//! Deserializers that accept what small models actually return.
//!
//! The analysis prompt asks for a fixed JSON shape, and a local 3B model obeys
//! it most of the time. When it does not, it is almost always a type slip rather
//! than a missing field: a list of bullet points where the schema asks for one
//! string, `"0.8"` where it asks for a number, `"yes"` where it asks for a bool.
//!
//! That mattered more than it looks. The whole analysis is one struct, so serde
//! rejecting a single field discarded every other field the model got right and
//! dropped the call to the regex summarizer — observed in the wild as
//! `LLM structured analysis error, using fallback: Structured JSON parsing
//! failed: invalid type: sequence, expected a string`, with no indication of
//! which field was at fault.
//!
//! The `serde_json::Value`-typed fields in `LlmRawAnalysis` were already
//! coerced by hand at the point of use, accepting array, object or string. These
//! give the plainly-typed fields the same tolerance instead of leaving them as
//! the one strict thing in an otherwise forgiving path.

use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// Flatten a JSON value into prose.
///
/// Arrays and object values are joined; a fragment that already ends in
/// sentence punctuation is joined with a space, anything else with `"; "`, so a
/// bullet list reads as a sentence without inventing punctuation that was not
/// there.
/// Whether a string is a model writing out "I have no value" instead of
/// emitting JSON null.
///
/// One archived call carried the literal four-character title "null" on its page
/// because of this. Matched only when the whole field is that word, so a title
/// like "none of the parts arrived" is left alone.
fn is_written_absence(value: &str) -> bool {
    matches!(
        value.to_lowercase().as_str(),
        "null" | "none" | "nil" | "n/a" | "na" | "undefined" | "unknown" | "-"
    )
}

fn to_prose(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() || is_written_absence(trimmed) {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(items) => join(items.iter()),
        Value::Object(map) => join(map.values()),
    }
}

fn join<'a>(parts: impl Iterator<Item = &'a Value>) -> Option<String> {
    let pieces: Vec<String> = parts.filter_map(to_prose).collect();
    if pieces.is_empty() {
        return None;
    }
    let mut out = String::new();
    for piece in pieces {
        if !out.is_empty() {
            let ends_a_sentence = out.ends_with(['.', '!', '?', '…', ':', ';']);
            out.push_str(if ends_a_sentence { " " } else { "; " });
        }
        out.push_str(&piece);
    }
    Some(out)
}

/// A field the schema declares as a string, whatever the model sent.
pub fn string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(to_prose))
}

/// A field the schema declares as a bool.
///
/// Accepts the words a model reaches for when asked whether something was
/// resolved, in either of the two languages this project runs in.
pub fn boolean<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(Value::Bool(b)) => Some(b),
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0),
        Some(Value::String(s)) => match s.trim().to_lowercase().as_str() {
            "true" | "yes" | "y" | "1" | "да" | "כן" => Some(true),
            "false" | "no" | "n" | "0" | "нет" | "לא" => Some(false),
            // Unrecognised wording falls through to the caller's default rather
            // than guessing a side.
            _ => None,
        },
        // Absent, null, or a shape a bool cannot come from.
        _ => None,
    })
}

/// A field the schema declares as a number.
pub fn number<'de, D>(deserializer: D) -> Result<Option<f32>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(Value::Number(n)) => n.as_f64().map(|f| f as f32),
        // Models quote numbers surprisingly often, and a sentiment score is the
        // field most likely to come back as `"0.8"` or `"+0.8"`.
        Some(Value::String(s)) => s.trim().trim_start_matches('+').parse().ok(),
        // Absent, null, or a shape a number cannot come from.
        _ => None,
    })
}

/// Describe a JSON value's shape, for a log line that says which field slipped.
///
/// The parse error serde produces names the *type* it wanted but not the field,
/// which is exactly the information needed to fix a prompt.
#[must_use]
pub fn describe_shape(value: &Value) -> String {
    let kind = |v: &Value| match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{k}={}", kind(v)))
            .collect::<Vec<_>>()
            .join(" "),
        other => format!("<top level is {}>", kind(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Probe {
        #[serde(default, deserialize_with = "string")]
        summary: Option<String>,
        #[serde(default, deserialize_with = "boolean")]
        resolved: Option<bool>,
        #[serde(default, deserialize_with = "number")]
        score: Option<f32>,
    }

    fn probe(json: &str) -> Probe {
        serde_json::from_str(json).expect("must not reject a type slip")
    }

    /// The exact failure seen in production: a list where a string was asked for.
    #[test]
    fn a_list_of_points_becomes_one_string() {
        let p = probe(r#"{"summary":["Клиент просил отменить заказ.","Согласовали возврат."]}"#);
        assert_eq!(
            p.summary.as_deref(),
            Some("Клиент просил отменить заказ. Согласовали возврат."),
            "fragments that end a sentence join with a space"
        );

        let p = probe(r#"{"summary":["отмена заказа","возврат средств"]}"#);
        assert_eq!(
            p.summary.as_deref(),
            Some("отмена заказа; возврат средств"),
            "fragments without punctuation must not have it invented for them"
        );
    }

    #[test]
    fn strings_pass_through_and_blanks_become_none() {
        assert_eq!(
            probe(r#"{"summary":" text "}"#).summary.as_deref(),
            Some("text")
        );
        assert!(probe(r#"{"summary":""}"#).summary.is_none());
        assert!(probe(r#"{"summary":"   "}"#).summary.is_none());
        assert!(probe(r#"{"summary":null}"#).summary.is_none());
        assert!(probe("{}").summary.is_none());
        assert!(probe(r#"{"summary":[]}"#).summary.is_none());
        // Nested structures flatten rather than failing.
        assert_eq!(
            probe(r#"{"summary":{"a":"one","b":["two","three"]}}"#)
                .summary
                .as_deref(),
            Some("one; two; three")
        );
    }

    #[test]
    fn booleans_accept_the_words_models_use() {
        for (json, expected) in [
            (r#"{"resolved":true}"#, Some(true)),
            (r#"{"resolved":"yes"}"#, Some(true)),
            (r#"{"resolved":"Да"}"#, Some(true)),
            (r#"{"resolved":"כן"}"#, Some(true)),
            (r#"{"resolved":1}"#, Some(true)),
            (r#"{"resolved":"no"}"#, Some(false)),
            (r#"{"resolved":"нет"}"#, Some(false)),
            (r#"{"resolved":0}"#, Some(false)),
            // Unrecognised text must fall through to the caller's default, not
            // guess a value.
            (r#"{"resolved":"partially"}"#, None),
            (r#"{"resolved":null}"#, None),
        ] {
            assert_eq!(probe(json).resolved, expected, "{json}");
        }
    }

    #[test]
    fn numbers_accept_quoted_values() {
        assert_eq!(probe(r#"{"score":0.8}"#).score, Some(0.8));
        assert_eq!(probe(r#"{"score":"0.8"}"#).score, Some(0.8));
        assert_eq!(probe(r#"{"score":"+0.8"}"#).score, Some(0.8));
        assert_eq!(probe(r#"{"score":"-0.5"}"#).score, Some(-0.5));
        assert_eq!(probe(r#"{"score":-1}"#).score, Some(-1.0));
        assert_eq!(probe(r#"{"score":"neutral"}"#).score, None);
        assert_eq!(probe(r#"{"score":[0.8]}"#).score, None);
    }

    #[test]
    fn shape_description_names_the_fields() {
        let value: Value =
            serde_json::from_str(r#"{"summary":["a"],"resolved":true,"score":0.5}"#).unwrap();
        let shape = describe_shape(&value);
        assert!(shape.contains("summary=array"), "{shape}");
        assert!(shape.contains("resolved=bool"), "{shape}");
        assert_eq!(
            describe_shape(&Value::Array(vec![])),
            "<top level is array>"
        );
    }
}

#[cfg(test)]
mod placeholder_word_tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Analysis {
        #[serde(default, deserialize_with = "string")]
        title: Option<String>,
    }

    /// A model asked for a value it does not have writes the word out as a
    /// string instead of emitting JSON null. One archived call has the literal
    /// four-character title "null" on its page because of it.
    #[test]
    fn the_word_a_model_writes_for_an_absent_value_counts_as_absent() {
        for written in [
            r#"{"title": "null"}"#,
            r#"{"title": "NULL"}"#,
            r#"{"title": "none"}"#,
            r#"{"title": "N/A"}"#,
            r#"{"title": "undefined"}"#,
            r#"{"title": null}"#,
            r#"{}"#,
        ] {
            let parsed: Analysis = serde_json::from_str(written).expect("parses");
            assert_eq!(parsed.title, None, "{written}");
        }
    }

    /// Only the bare word, so real text is never thrown away.
    #[test]
    fn a_title_that_merely_contains_the_word_survives() {
        for (written, expected) in [
            (
                r#"{"title": "Nullsoft installer support call"}"#,
                "Nullsoft installer support call",
            ),
            (
                r#"{"title": "none of the parts arrived"}"#,
                "none of the parts arrived",
            ),
        ] {
            let parsed: Analysis = serde_json::from_str(written).expect("parses");
            assert_eq!(parsed.title.as_deref(), Some(expected));
        }
    }
}
