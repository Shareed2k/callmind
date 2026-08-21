use thiserror::Error;

#[derive(Debug, Error)]
pub enum SttError {
    #[error("Speech-to-text inference failed: {0}")]
    Inference(String),

    #[error("STT Model loading failed at '{path}': {message}")]
    ModelLoad { path: String, message: String },

    #[error("Empty or invalid audio input for transcription")]
    InvalidAudio,

    #[error("GPU Out-of-Memory / CUDA error: {0}")]
    GpuOom(String),
}
