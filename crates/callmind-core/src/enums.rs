use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CallDirection {
    Incoming,
    Outgoing,
    Internal,
    Unknown,
}

impl CallDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Incoming => "incoming",
            Self::Outgoing => "outgoing",
            Self::Internal => "internal",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for CallDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for CallDirection {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "incoming" => Self::Incoming,
            "outgoing" => Self::Outgoing,
            "internal" => Self::Internal,
            _ => Self::Unknown,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

impl ProcessingStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for ProcessingStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ProcessingStatus {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "processing" => Self::Processing,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Pending,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for JobStatus {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Pending,
        })
    }
}

/// A kind of background job.
///
/// `Custom` is the extension point: a closed-source plugin registers a handler
/// under its own name without this enum being patched. The `plugin:` prefix keeps
/// the two namespaces apart, so a plugin can never take over a built-in kind.
///
/// Not `Copy` any more, because a plugin name is a `String`. There was one call
/// site relying on that.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JobKind {
    IngestRecording,
    DecodeAudio,
    DetectLanguage,
    Transcribe,
    Diarize,
    BuildTranscript,
    NormalizeTranscript,
    AnalyzeCall,
    AnalyzeEmotions,
    DeliverWebhook,
    /// A stage supplied by a plugin, named by the plugin.
    Custom(String),
}

/// Prefix that separates plugin kinds from built-in ones.
const PLUGIN_KIND_PREFIX: &str = "plugin:";

// Serialized as the same string the database stores, rather than serde's default
// externally-tagged form for a newtype variant. One representation everywhere is
// worth fifteen lines: two would drift, and the drift would be silent.
impl Serialize for JobKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_str())
    }
}

// The schema is written out for the same reason serde is: this is a string on
// the wire, and a derived enum schema would describe a shape that never travels.
impl utoipa::PartialSchema for JobKind {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .description(Some(
                "Job kind. A built-in name such as `ingest_recording`, or \
                 `plugin:<name>` for a stage supplied by a plugin.",
            ))
            .into()
    }
}

impl utoipa::ToSchema for JobKind {}

impl<'de> Deserialize<'de> for JobKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl JobKind {
    /// The stored form, used both in the database and on the wire.
    ///
    /// Borrowed for a built-in kind and owned only for a plugin one, so the
    /// common path still allocates nothing.
    #[must_use]
    pub fn as_str(&self) -> std::borrow::Cow<'static, str> {
        use std::borrow::Cow;
        match self {
            Self::IngestRecording => Cow::Borrowed("ingest_recording"),
            Self::DecodeAudio => Cow::Borrowed("decode_audio"),
            Self::DetectLanguage => Cow::Borrowed("detect_language"),
            Self::Transcribe => Cow::Borrowed("transcribe"),
            Self::Diarize => Cow::Borrowed("diarize"),
            Self::BuildTranscript => Cow::Borrowed("build_transcript"),
            Self::NormalizeTranscript => Cow::Borrowed("normalize_transcript"),
            Self::AnalyzeCall => Cow::Borrowed("analyze_call"),
            Self::AnalyzeEmotions => Cow::Borrowed("analyze_emotions"),
            Self::DeliverWebhook => Cow::Borrowed("deliver_webhook"),
            Self::Custom(name) => Cow::Owned(format!("{PLUGIN_KIND_PREFIX}{name}")),
        }
    }

    /// Whether this kind comes from a plugin rather than the core pipeline.
    #[must_use]
    pub fn is_plugin(&self) -> bool {
        matches!(self, Self::Custom(_))
    }
}

impl fmt::Display for JobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for JobKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ingest_recording" => Ok(Self::IngestRecording),
            "decode_audio" => Ok(Self::DecodeAudio),
            "detect_language" => Ok(Self::DetectLanguage),
            "transcribe" => Ok(Self::Transcribe),
            "diarize" => Ok(Self::Diarize),
            "build_transcript" => Ok(Self::BuildTranscript),
            "normalize_transcript" => Ok(Self::NormalizeTranscript),
            "analyze_call" => Ok(Self::AnalyzeCall),
            "analyze_emotions" => Ok(Self::AnalyzeEmotions),
            "deliver_webhook" => Ok(Self::DeliverWebhook),
            other => match other.strip_prefix(PLUGIN_KIND_PREFIX) {
                // An empty plugin name is a bug at the registration site, not a
                // kind: accepting it would create an unaddressable handler.
                Some("") | None => Err(format!("Unknown job kind: {other}")),
                Some(name) => Ok(Self::Custom(name.to_string())),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerRole {
    Speaker1,
    Speaker2,
    Participant,
    Agent,
    Customer,
    Supervisor,
    Unknown,
}

impl SpeakerRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Speaker1 => "speaker_1",
            Self::Speaker2 => "speaker_2",
            Self::Participant => "participant",
            Self::Agent => "agent",
            Self::Customer => "customer",
            Self::Supervisor => "supervisor",
            Self::Unknown => "unknown",
        }
    }

    pub fn display_label(&self, speaker_id: Option<u16>) -> String {
        match self {
            Self::Speaker1 | Self::Agent => "Speaker 1".to_string(),
            Self::Speaker2 | Self::Customer => "Speaker 2".to_string(),
            Self::Supervisor => "Speaker 3".to_string(),
            Self::Participant => speaker_id.map_or_else(
                || "Participant".to_string(),
                |id| format!("Speaker {}", id + 1),
            ),
            Self::Unknown => {
                speaker_id.map_or_else(|| "Speaker".to_string(), |id| format!("Speaker {}", id + 1))
            }
        }
    }
}

impl fmt::Display for SpeakerRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_label(None))
    }
}

impl FromStr for SpeakerRole {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "speaker_1" | "speaker1" | "speaker 1" => Self::Speaker1,
            "speaker_2" | "speaker2" | "speaker 2" => Self::Speaker2,
            "participant" => Self::Participant,
            "agent" => Self::Agent,
            "customer" => Self::Customer,
            "supervisor" => Self::Supervisor,
            _ => Self::Unknown,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Hebrew,
    Russian,
    English,
    Arabic,
    Other(String),
    Unknown,
}

impl Language {
    pub fn code(&self) -> &str {
        match self {
            Self::Hebrew => "he",
            Self::Russian => "ru",
            Self::English => "en",
            Self::Arabic => "ar",
            Self::Other(code) => code.as_str(),
            Self::Unknown => "und",
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code())
    }
}

impl FromStr for Language {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "he" | "heb" | "hebrew" => Self::Hebrew,
            "ru" | "rus" | "russian" => Self::Russian,
            "en" | "eng" | "english" => Self::English,
            "ar" | "ara" | "arabic" => Self::Arabic,
            "und" | "unknown" => Self::Unknown,
            other => Self::Other(other.to_string()),
        })
    }
}

#[cfg(test)]
mod job_kind_custom_tests {
    use super::*;

    /// A closed-source plugin must be able to register a job kind without
    /// patching this enum. The `plugin:` prefix keeps a plugin's name from ever
    /// colliding with a built-in one.
    #[test]
    fn a_plugin_kind_round_trips_through_its_string_form() {
        let kind = JobKind::Custom("acoustic_emotions".to_string());
        assert_eq!(kind.as_str(), "plugin:acoustic_emotions");
        assert_eq!(
            "plugin:acoustic_emotions".parse::<JobKind>().unwrap(),
            kind,
            "the stored string must parse back to the same kind"
        );
    }

    #[test]
    fn built_in_kinds_still_round_trip() {
        for kind in [
            JobKind::IngestRecording,
            JobKind::AnalyzeCall,
            JobKind::DeliverWebhook,
        ] {
            let text = kind.as_str().to_string();
            assert_eq!(text.parse::<JobKind>().unwrap(), kind, "{text}");
        }
    }

    /// A plugin name must not be able to impersonate a built-in kind, and an
    /// empty one is a bug rather than a kind.
    #[test]
    fn a_plugin_kind_cannot_shadow_a_built_in_or_be_empty() {
        assert_eq!(
            "plugin:ingest_recording".parse::<JobKind>().unwrap(),
            JobKind::Custom("ingest_recording".to_string()),
            "the prefix keeps the namespaces apart"
        );
        assert!("plugin:".parse::<JobKind>().is_err());
        assert!("unknown_thing".parse::<JobKind>().is_err());
    }

    /// Registry lookup is by value, so two plugin kinds with the same name must
    /// be the same key and different names must not be.
    #[test]
    fn plugin_kinds_compare_and_hash_by_name() {
        use std::collections::HashMap;
        let mut map: HashMap<JobKind, u8> = HashMap::new();
        map.insert(JobKind::Custom("a".into()), 1);
        map.insert(JobKind::Custom("a".into()), 2);
        map.insert(JobKind::Custom("b".into()), 3);
        assert_eq!(map.len(), 2);
        assert_eq!(map[&JobKind::Custom("a".into())], 2);
    }
}
