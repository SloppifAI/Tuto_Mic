use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub fn dirs() -> ProjectDirs {
    ProjectDirs::from("de", "Tuto", "tuto-mic").expect("kein Home-Verzeichnis gefunden")
}

pub fn models_dir() -> PathBuf {
    dirs().data_dir().join("models")
}

/// Persistente Einstellungen (`<config_dir>/config.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub whisper_model: PathBuf,
    pub vad_model: PathBuf,
    pub language: String,
    pub ollama_url: String,
    pub ollama_model: String,
    pub num_ctx: u32,
    /// Länge der Aufnahme-Segmente. Bestimmt die Genauigkeit von "letzte X min löschen".
    pub segment_secs: u32,
    pub delete_audio_after_transcription: bool,
    /// Namen der Klasse (eine Person pro Eintrag, "Vorname Nachname") für die Pseudonymisierung.
    pub class_list: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            whisper_model: models_dir().join(crate::models::WHISPER.file),
            vad_model: models_dir().join(crate::models::VAD.file),
            language: "de".into(),
            ollama_url: "http://localhost:11434".into(),
            ollama_model: "qwen3.5:9b".into(),
            num_ctx: 32768,
            segment_secs: 10,
            delete_audio_after_transcription: true,
            class_list: Vec::new(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs().config_dir().join("config.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("Konfiguration nicht schreibbar: {}", path.display()))
    }
}
