use crate::errors::DiarizationError;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tract_onnx::prelude::*;

/// Neural Speaker Embedding Extractor using pure-Rust ONNX inference (`tract-onnx`).
/// Compatible with ECAPA-TDNN, PyAnnote embedding, and WeSpeaker ResNet models.
pub struct OnnxSpeakerEmbeddingExtractor {
    model_path: PathBuf,
    plan: Arc<TypedRunnableModel>,
}

impl OnnxSpeakerEmbeddingExtractor {
    /// Load an ONNX model from a given filesystem path.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, DiarizationError> {
        let path_buf = path.as_ref().to_path_buf();
        if !path_buf.exists() {
            return Err(DiarizationError::Inference(format!(
                "ONNX model file not found: {}",
                path_buf.display()
            )));
        }

        let model = tract_onnx::onnx()
            .model_for_path(&path_buf)
            .map_err(|e| DiarizationError::Inference(format!("Failed to parse ONNX model: {e}")))?
            .into_typed()
            .map_err(|e| {
                DiarizationError::Inference(format!("Failed to type-check ONNX model: {e}"))
            })?
            .into_decluttered()
            .map_err(|e| {
                DiarizationError::Inference(format!("Failed to declutter ONNX model: {e}"))
            })?
            .into_optimized()
            .map_err(|e| {
                DiarizationError::Inference(format!("Failed to optimize ONNX model: {e}"))
            })?
            .into_runnable()
            .map_err(|e| {
                DiarizationError::Inference(format!("Failed to build runnable plan: {e}"))
            })?;

        Ok(Self {
            model_path: path_buf,
            plan: model,
        })
    }

    /// Extract an L2-normalized speaker embedding vector from a 16kHz mono audio slice.
    pub fn extract_embedding(&self, samples: &[f32]) -> Result<Vec<f32>, DiarizationError> {
        if samples.is_empty() {
            return Err(DiarizationError::EmptyAudio);
        }

        // Standard 2D shape [1, samples]
        let input_tensor: Tensor =
            tract_ndarray::Array2::from_shape_vec((1, samples.len()), samples.to_vec())
                .map_err(|e| {
                    DiarizationError::Inference(format!("Failed to create input tensor shape: {e}"))
                })?
                .into();

        let output = self
            .plan
            .run(tvec!(input_tensor.into()))
            .map_err(|e| DiarizationError::Inference(format!("ONNX inference error: {e}")))?;

        if output.is_empty() {
            return Err(DiarizationError::Inference(
                "ONNX model produced no output tensor".to_string(),
            ));
        }

        let raw_view = output[0].to_plain_array_view::<f32>().map_err(|e| {
            DiarizationError::Inference(format!("Failed to read output tensor as f32 array: {e}"))
        })?;

        let mut embedding: Vec<f32> = raw_view.iter().copied().collect();

        // L2 normalize the embedding vector
        let norm_sq: f32 = embedding.iter().map(|&x| x * x).sum();
        if norm_sq > 1e-8 {
            let norm = norm_sq.sqrt();
            for val in &mut embedding {
                *val /= norm;
            }
        }

        Ok(embedding)
    }

    #[must_use]
    pub fn model_path(&self) -> &Path {
        &self.model_path
    }
}
