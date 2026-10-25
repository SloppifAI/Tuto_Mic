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
        std::fs::create_dir_all(path.parent().expect("Konfigurationspfad liegt in einem Verzeichnis"))?;
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("Konfiguration nicht schreibbar: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let cfg: Config = serde_json::from_str(r#"{"ollama_model": "nimble:9b"}"#).unwrap();
        assert_eq!(cfg.ollama_model, "nimble:9b");
        assert_eq!(cfg.language, "de");
        assert_eq!(cfg.segment_secs, 10);
        assert!(cfg.delete_audio_after_transcription);
        assert_eq!(cfg.class_list, Vec::<String>::new());
    }

    #[test]
    fn roundtrips_through_json() {
        let cfg = Config {
            class_list: vec!["Lena Wagner".into()],
            ..Config::default()
        };
        let back: Config = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.class_list, cfg.class_list);
        assert_eq!(back.whisper_model, cfg.whisper_model);
    }
}
