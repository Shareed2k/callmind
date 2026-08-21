use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// Parsed metadata extracted from call recording filenames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ParsedCallFilename {
    /// Original filename.
    pub raw_filename: String,
    /// Extracted contact name (if caller is named).
    pub contact_name: Option<String>,
    /// Extracted phone number (if numeric/international format).
    pub phone_number: Option<String>,
    /// Extracted call start timestamp.
    pub started_at: Option<DateTime<Utc>>,
}

/// Utility for extracting caller information, dates, and times from call audio filenames.
pub struct CallFilenameParser;

impl CallFilenameParser {
    /// Parse a filename (e.g. "Call recording סמי אינסטלטור_250820_142338.m4a" or "Call +972507361902_260705_142254.m4a").
    #[must_use]
    pub fn parse(filename: &str) -> ParsedCallFilename {
        let name_without_ext = filename.rsplit_once('.').map_or(filename, |(base, _)| base);

        // Strip known prefixes
        let clean_name = if let Some(stripped) = name_without_ext.strip_prefix("Call recording ") {
            stripped
        } else if let Some(stripped) = name_without_ext.strip_prefix("Call ") {
            stripped
        } else {
            name_without_ext
        };

        // Split by '_' to extract date and time components
        let parts: Vec<&str> = clean_name.split('_').collect();

        let mut started_at = None;
        let mut caller_identifier = clean_name.to_string();

        if parts.len() >= 3 {
            let time_part = parts[parts.len() - 1];
            let date_part = parts[parts.len() - 2];

            if let Some(parsed_dt) = parse_yymmdd_hhmmss(date_part, time_part) {
                started_at = Some(parsed_dt);
                // The caller identifier is everything before the date part
                caller_identifier = parts[..parts.len() - 2].join("_");
            }
        }

        let is_phone = is_phone_number(&caller_identifier);
        let phone_number = if is_phone {
            Some(caller_identifier.clone())
        } else {
            None
        };

        let contact_name = if !is_phone && !caller_identifier.is_empty() {
            Some(caller_identifier)
        } else {
            None
        };

        ParsedCallFilename {
            raw_filename: filename.to_string(),
            contact_name,
            phone_number,
            started_at,
        }
    }
}

/// Parse YYMMDD and HHMMSS strings into `DateTime<Utc>`.
fn parse_yymmdd_hhmmss(date_str: &str, time_str: &str) -> Option<DateTime<Utc>> {
    if date_str.len() != 6 || time_str.len() != 6 {
        return None;
    }

    let yy: i32 = date_str[0..2].parse().ok()?;
    let mm: u32 = date_str[2..4].parse().ok()?;
    let dd: u32 = date_str[4..6].parse().ok()?;

    let hour: u32 = time_str[0..2].parse().ok()?;
    let min: u32 = time_str[2..4].parse().ok()?;
    let sec: u32 = time_str[4..6].parse().ok()?;

    let year = 2000 + yy;
    let date = NaiveDate::from_ymd_opt(year, mm, dd)?;
    let time = NaiveTime::from_hms_opt(hour, min, sec)?;
    let naive_dt = NaiveDateTime::new(date, time);

    Some(Utc.from_utc_datetime(&naive_dt))
}

/// Heuristic checking if a string represents a phone number.
fn is_phone_number(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }

    let digits_count = trimmed.chars().filter(char::is_ascii_digit).count();
    let is_numeric = trimmed
        .chars()
        .all(|c| c.is_ascii_digit() || c == '+' || c == '-' || c == ' ' || c == '(' || c == ')');

    is_numeric && digits_count >= 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;
    use chrono::Timelike;

    #[test]
    fn test_parse_hebrew_contact_recording() {
        let filename = "Call recording סמי אינסטלטור_250820_142338.m4a";
        let parsed = CallFilenameParser::parse(filename);

        assert_eq!(parsed.contact_name.as_deref(), Some("סמי אינסטלטור"));
        assert_eq!(parsed.phone_number, None);

        let dt = parsed.started_at.unwrap();
        assert_eq!(dt.year(), 2025);
        assert_eq!(dt.month(), 8);
        assert_eq!(dt.day(), 20);
        assert_eq!(dt.hour(), 14);
        assert_eq!(dt.minute(), 23);
        assert_eq!(dt.second(), 38);
    }

    #[test]
    fn test_parse_international_phone_call() {
        let filename = "Call +972507361902_260705_142254.m4a";
        let parsed = CallFilenameParser::parse(filename);

        assert_eq!(parsed.phone_number.as_deref(), Some("+972507361902"));
        assert_eq!(parsed.contact_name, None);

        let dt = parsed.started_at.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 7);
        assert_eq!(dt.day(), 5);
        assert_eq!(dt.hour(), 14);
        assert_eq!(dt.minute(), 22);
    }

    #[test]
    fn test_parse_russian_contact_recording() {
        let filename = "Call recording Шамиль Работа Bringg_240513_111449.m4a";
        let parsed = CallFilenameParser::parse(filename);

        assert_eq!(parsed.contact_name.as_deref(), Some("Шамиль Работа Bringg"));
        assert_eq!(parsed.phone_number, None);

        let dt = parsed.started_at.unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 5);
        assert_eq!(dt.day(), 13);
        assert_eq!(dt.hour(), 11);
        assert_eq!(dt.minute(), 14);
    }
}
