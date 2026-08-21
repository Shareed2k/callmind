use crate::errors::LanguageError;
use crate::models::{LanguageDetection, LanguageProbability};
use crate::traits::LanguageEngine;
use async_trait::async_trait;
use callmind_audio::AudioBuffer;
use callmind_core::Language;
use callmind_vad::SpeechRegion;

use std::sync::Arc;

pub type DetectorFn = Arc<dyn Fn(&AudioBuffer) -> Vec<LanguageProbability> + Send + Sync>;

/// Multi-window sampling language detection engine.
#[derive(Clone)]
pub struct SamplingLanguageEngine {
    pub window_duration_ms: u64,
    pub max_samples: usize,
    pub mixed_threshold: f32,
    detector: Option<DetectorFn>,
}

impl std::fmt::Debug for SamplingLanguageEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SamplingLanguageEngine")
            .field("window_duration_ms", &self.window_duration_ms)
            .field("max_samples", &self.max_samples)
            .field("mixed_threshold", &self.mixed_threshold)
            .field("has_custom_detector", &self.detector.is_some())
            .finish()
    }
}

impl Default for SamplingLanguageEngine {
    fn default() -> Self {
        Self {
            window_duration_ms: 15_000,
            max_samples: 4,
            mixed_threshold: 0.15,
            detector: None,
        }
    }
}

impl SamplingLanguageEngine {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_detector<F>(mut self, detector: F) -> Self
    where
        F: Fn(&AudioBuffer) -> Vec<LanguageProbability> + Send + Sync + 'static,
    {
        self.detector = Some(Arc::new(detector));
        self
    }

    /// Helper calculating language distribution from multiple window detections.
    pub fn aggregate_distributions(
        sample_probs: &[Vec<LanguageProbability>],
        mixed_threshold: f32,
    ) -> LanguageDetection {
        if sample_probs.is_empty() {
            return LanguageDetection::new(
                Language::Unknown,
                vec![LanguageProbability {
                    language: Language::Unknown,
                    probability: 1.0,
                }],
                false,
            );
        }

        let mut lang_totals: std::collections::HashMap<Language, f32> =
            std::collections::HashMap::new();
        let total_samples = sample_probs.len() as f32;

        for sample in sample_probs {
            for lp in sample {
                *lang_totals.entry(lp.language.clone()).or_insert(0.0) += lp.probability;
            }
        }

        let mut distribution: Vec<LanguageProbability> = lang_totals
            .into_iter()
            .map(|(language, sum)| LanguageProbability {
                language,
                probability: sum / total_samples,
            })
            .collect();

        distribution.sort_by(|a, b| {
            b.probability
                .partial_cmp(&a.probability)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let primary = distribution
            .first()
            .map(|lp| lp.language.clone())
            .unwrap_or(Language::Unknown);

        let second_prob = distribution.get(1).map_or(0.0, |lp| lp.probability);
        let mixed = second_prob >= mixed_threshold;

        LanguageDetection::new(primary, distribution, mixed)
    }
}

#[async_trait]
impl LanguageEngine for SamplingLanguageEngine {
    async fn detect(
        &self,
        audio: &AudioBuffer,
        speech_regions: &[SpeechRegion],
    ) -> Result<LanguageDetection, LanguageError> {
        if audio.is_empty() {
            return Err(LanguageError::EmptyAudio);
        }

        // Extract speech windows to sample
        let mut sample_windows: Vec<AudioBuffer> = Vec::new();

        if speech_regions.is_empty() {
            // Sample evenly across entire audio
            let duration = audio.duration_ms();
            let step = (duration / (self.max_samples as u64 + 1)).max(1000);
            for i in 1..=self.max_samples {
                let start_ms = (i as u64) * step;
                let end_ms = (start_ms + self.window_duration_ms).min(duration);
                if start_ms < end_ms {
                    sample_windows.push(audio.slice_time(start_ms, end_ms));
                }
            }
        } else {
            // Sample from detected speech regions
            let regions_to_take = speech_regions.iter().take(self.max_samples);
            for region in regions_to_take {
                let start_ms = region.start_ms;
                let end_ms = (region.start_ms + self.window_duration_ms).min(region.end_ms);
                if start_ms < end_ms {
                    sample_windows.push(audio.slice_time(start_ms, end_ms));
                }
            }
        }

        if sample_windows.is_empty() {
            sample_windows.push(audio.clone());
        }

        // Dynamic multi-window language identification
        let mut samples_res: Vec<Vec<LanguageProbability>> = Vec::new();
        for window in &sample_windows {
            if let Some(ref det) = self.detector {
                let res = det(window);
                if !res.is_empty() {
                    samples_res.push(res);
                    continue;
                }
            }

            // Signal-level fallback when running without dedicated acoustic detector;
            // actual language classification is confirmed during STT decoding.
            samples_res.push(vec![LanguageProbability {
                language: Language::Unknown,
                probability: 1.0,
            }]);
        }

        Ok(Self::aggregate_distributions(
            &samples_res,
            self.mixed_threshold,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aggregate_distributions_mixed() {
        let sample1 = vec![
            LanguageProbability {
                language: Language::Hebrew,
                probability: 0.60,
            },
            LanguageProbability {
                language: Language::Russian,
                probability: 0.40,
            },
        ];
        let sample2 = vec![
            LanguageProbability {
                language: Language::Hebrew,
                probability: 0.80,
            },
            LanguageProbability {
                language: Language::Russian,
                probability: 0.20,
            },
        ];

        let detection = SamplingLanguageEngine::aggregate_distributions(&[sample1, sample2], 0.15);
        assert_eq!(detection.primary, Language::Hebrew);
        assert!((detection.hebrew_ratio() - 0.70).abs() < 0.01);
        assert!((detection.russian_ratio() - 0.30).abs() < 0.01);
        assert!(detection.mixed);
    }

    #[tokio::test]
    async fn test_sampling_language_engine_with_custom_detector() {
        let engine = SamplingLanguageEngine::new().with_detector(|_buf| {
            vec![
                LanguageProbability {
                    language: Language::Russian,
                    probability: 0.90,
                },
                LanguageProbability {
                    language: Language::English,
                    probability: 0.10,
                },
            ]
        });

        let audio = AudioBuffer::new(16000, 1, vec![0.1; 16000]);
        let det = engine.detect(&audio, &[]).await.unwrap();
        assert_eq!(det.primary, Language::Russian);
        assert!((det.confidence() - 0.90).abs() < 0.01);
    }
}
