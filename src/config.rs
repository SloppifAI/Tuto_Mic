//! Persistente Einstellungen und App-Verzeichnisse.

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

    /// Lädt `config.json`. Fehlt die Datei, gelten die Standardwerte. Eine fehlerhafte Datei ist ein Fehler und
    /// wird nicht still durch Standardwerte ersetzt – sonst ginge unbemerkt die Klassenliste und damit die
    /// Pseudonymisierung verloren.
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).with_context(|| format!("Konfiguration fehlerhaft: {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("Konfiguration nicht lesbar: {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        std::fs::create_dir_all(path.parent().expect("Konfigurationspfad liegt in einem Verzeichnis"))?;
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("Konfiguration nicht schreibbar: {}", path.display()))
    }
}

/// Eine Person pro Zeile; Leerzeilen und Ränder werden entfernt.
pub fn parse_class_list(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_distinguishes_missing_and_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        assert_eq!(Config::load_from(&path).unwrap().language, "de");

        std::fs::write(&path, r#"{"class_list": ["Lena Wagner""#).unwrap();
        let err = Config::load_from(&path).unwrap_err();
        assert!(format!("{err:#}").contains("Konfiguration fehlerhaft"), "{err:#}");

        std::fs::write(&path, r#"{"segment_secs": "zehn"}"#).unwrap();
        assert!(Config::load_from(&path).is_err());
    }

    #[test]
    fn parses_class_list() {
        assert_eq!(
            parse_class_list(" Lena Wagner \n\n Tom Becker\n"),
            ["Lena Wagner", "Tom Becker"]
        );
    }

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
