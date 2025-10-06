use std::{env, fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ModelsConfig {
    pub modules: ModulesConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ModulesConfig {
    pub summarizer: Option<SummarizerModels>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SummarizerModels {
    pub summary_model: String,
    pub translation_model: String,
}

impl ModelsConfig {
    pub fn load_default() -> Result<Self> {
        let path =
            env::var("MODELS_CONFIG_PATH").unwrap_or_else(|_| "config/models.yaml".to_string());
        Self::load_from_path(path)
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path).with_context(|| {
            format!(
                "failed to read models configuration from {}",
                path.display()
            )
        })?;

        let config: ModelsConfig = serde_yaml::from_str(&contents).with_context(|| {
            format!("failed to parse models configuration at {}", path.display())
        })?;

        Ok(config)
    }

    pub fn summarizer(&self) -> Option<&SummarizerModels> {
        self.modules.summarizer.as_ref()
    }
}

impl SummarizerModels {
    pub fn summary_model(&self) -> &str {
        &self.summary_model
    }

    pub fn translation_model(&self) -> &str {
        &self.translation_model
    }
}
