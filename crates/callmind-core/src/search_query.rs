//! Free-text query expansion, shared by every full-text backend.
//!
//! Lives here rather than in `callmind-search` because the repositories need it
//! too, and `callmind-search` depends on `callmind-db`. The expansion itself is
//! pure text work with no database in it.
//!
//! Two rules, both learned from real recordings:
//!
//! - Every token becomes a **prefix** term, so a stem still matches an inflected
//!   form. Russian inflects by suffix: `разгов` has to find "разговор".
//! - Hebrew attaches its article and prepositions to the **front** of a word, and
//!   no full-text engine's prefix term can match `השיחה` from `שיחה`. So a Hebrew
//!   token is additionally emitted with each proclitic attached. Verified to be
//!   necessary on both SQLite FTS5 and Postgres `tsvector` — neither matches the
//!   bare stem without it.

/// The prefixes Hebrew attaches directly to a word: the definite article, and
/// the conjunction and prepositions that behave the same way.
pub const HEBREW_PROCLITICS: [&str; 7] = ["ה", "ו", "ב", "ל", "מ", "כ", "ש"];

fn is_hebrew(text: &str) -> bool {
    text.chars().any(|c| ('\u{0590}'..='\u{05FF}').contains(&c))
}

/// Split a user query into search terms, expanded for morphology.
///
/// The returned strings are bare tokens; each backend wraps them in its own
/// prefix syntax.
#[must_use]
pub fn expand_terms(query: &str) -> Vec<String> {
    // Characters that mean something to FTS5 or to `tsquery` are separators
    // rather than input, so neither dialect can be injected through the box.
    let tokens = query.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '*' | ':' | '(' | ')' | '\'' | '&' | '|' | '!' | '<' | '>' | '\\'
            )
    });

    let mut terms = Vec::new();
    for token in tokens.filter(|s| !s.is_empty()) {
        terms.push(token.to_string());
        if is_hebrew(token) {
            for proclitic in HEBREW_PROCLITICS {
                terms.push(format!("{proclitic}{token}"));
            }
        }
    }
    terms
}

/// Render a query as SQLite FTS5 syntax: `"term"* OR "term"*`.
#[must_use]
pub fn to_fts5(query: &str) -> String {
    expand_terms(query)
        .iter()
        .map(|t| format!("\"{t}\"*"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Render a query as a Postgres `tsquery`: `term:* | term:*`.
#[must_use]
pub fn to_tsquery(query: &str) -> String {
    expand_terms(query)
        .iter()
        .map(|t| format!("{t}:*"))
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_languages_need_only_a_prefix_term() {
        assert_eq!(to_fts5("отмена заказа"), "\"отмена\"* OR \"заказа\"*");
        assert_eq!(to_tsquery("отмена заказа"), "отмена:* | заказа:*");
    }

    /// Both dialects must carry the proclitic expansion, because neither can
    /// match `השיחה` from the bare stem on its own.
    #[test]
    fn hebrew_is_expanded_in_both_dialects() {
        for stem in ["הזמנה", "דחופה"] {
            let fts5 = to_fts5(stem);
            let ts = to_tsquery(stem);
            for proclitic in HEBREW_PROCLITICS {
                assert!(
                    fts5.contains(&format!("\"{proclitic}{stem}\"*")),
                    "fts5 missing {proclitic}{stem}: {fts5}"
                );
                assert!(
                    ts.contains(&format!("{proclitic}{stem}:*")),
                    "tsquery missing {proclitic}{stem}: {ts}"
                );
            }
        }
    }

    #[test]
    fn operators_are_separators_not_input() {
        assert_eq!(to_fts5(""), "");
        assert_eq!(to_fts5("   "), "");
        assert_eq!(
            to_fts5("a\"b*c:d(e)"),
            "\"a\"* OR \"b\"* OR \"c\"* OR \"d\"* OR \"e\"*"
        );
        // `&`, `|` and `!` are tsquery operators, and `'` would close the
        // literal a caller might interpolate it into.
        assert_eq!(to_tsquery("a&b|c!d'e"), "a:* | b:* | c:* | d:* | e:*");
    }
}
