use crate::models::{ClassifierResult, Evidence};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Data type of the classifier result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClassifierOutputType {
    Boolean,
    Enum(Vec<String>),
    Number,
    String,
}

/// Custom configurable business classifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Classifier {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub output_type: ClassifierOutputType,
    pub enabled: bool,
}

impl Classifier {
    pub fn new_boolean(name: &str, description: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.to_string(),
            description: description.to_string(),
            output_type: ClassifierOutputType::Boolean,
            enabled: true,
        }
    }

    pub fn new_enum(name: &str, description: &str, options: Vec<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.to_string(),
            description: description.to_string(),
            output_type: ClassifierOutputType::Enum(options),
            enabled: true,
        }
    }

    /// Evaluates basic keywords if running lightweight heuristics without LLM.
    pub fn evaluate_heuristic(&self, text: &str) -> Option<ClassifierResult> {
        let lower = text.to_lowercase();
        match &self.output_type {
            ClassifierOutputType::Boolean => {
                let matches = if self.name.to_lowercase().contains("cancellation") {
                    lower.contains("לבטל")
                        || lower.contains("ביטול")
                        || lower.contains("отменить")
                        || lower.contains("отмена")
                        || lower.contains("cancel")
                } else if self.name.to_lowercase().contains("complaint") {
                    lower.contains("תלונה")
                        || lower.contains("גרוע")
                        || lower.contains("жалоба")
                        || lower.contains("ужасно")
                        || lower.contains("bad service")
                } else {
                    false
                };

                Some(ClassifierResult {
                    classifier_id: self.id,
                    name: self.name.clone(),
                    value: serde_json::Value::Bool(matches),
                    evidence: Evidence::default(),
                })
            }
            _ => None,
        }
    }
}
