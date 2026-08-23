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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
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
}

impl JobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::IngestRecording => "ingest_recording",
            Self::DecodeAudio => "decode_audio",
            Self::DetectLanguage => "detect_language",
            Self::Transcribe => "transcribe",
            Self::Diarize => "diarize",
            Self::BuildTranscript => "build_transcript",
            Self::NormalizeTranscript => "normalize_transcript",
            Self::AnalyzeCall => "analyze_call",
            Self::AnalyzeEmotions => "analyze_emotions",
            Self::DeliverWebhook => "deliver_webhook",
        }
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
            other => Err(format!("Unknown job kind: {other}")),
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
