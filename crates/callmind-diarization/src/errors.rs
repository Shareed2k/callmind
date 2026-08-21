use thiserror::Error;

#[derive(Debug, Error)]
pub enum DiarizationError {
    #[error("Diarization inference error: {0}")]
    Inference(String),

    #[error("Empty audio supplied to Diarizer")]
    EmptyAudio,

    #[error("VAD error during diarization: {0}")]
    Vad(#[from] callmind_vad::VadError),
}
