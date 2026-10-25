//! Einstellungen und App-Verzeichnisse.
//!
//! Lehrkräfte sehen keine Einstellungen. Es gelten die Standardwerte; Betreuer ändern sie zentral über eine
//! Datei (`config.json`, Pfad über `TUTO_MIC_CONFIG` wählbar) und/oder Umgebungsvariablen `TUTO_MIC_*`, die
//! Vorrang vor der Datei haben (siehe [`ENV_VARS`]).

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

/// Umgebungsvariablen und was sie setzen (für Doku und `tuto-mic info`).
pub const ENV_VARS: &[(&str, &str)] = &[
    ("TUTO_MIC_TUTO_URL", "Adresse der Tuto-Instanz"),
    ("TUTO_MIC_SUMMARY_LIMIT", "Höchstlänge der Zusammenfassung in Zeichen"),
    ("TUTO_MIC_OLLAMA_URL", "Adresse von Ollama"),
    ("TUTO_MIC_OLLAMA_MODEL", "Sprachmodell für die Zusammenfassung"),
    ("TUTO_MIC_LANGUAGE", "Sprache für Whisper, z. B. de"),
    ("TUTO_MIC_WHISPER_MODEL", "Pfad zum Whisper-Modell"),
    ("TUTO_MIC_VAD_MODEL", "Pfad zum VAD-Modell"),
    (
        "TUTO_MIC_DELETE_AUDIO",
        "Audio nach der Transkription löschen (true/false)",
    ),
    (
        "TUTO_MIC_KEEP_SESSIONS",
        "Daten nach dem Senden an Tuto behalten (true/false)",
    ),
    ("TUTO_MIC_CLASS_LIST", "Klassenliste, Namen mit ; getrennt"),
];

/// Einstellungen, zusammengesetzt aus Standardwerten, Datei und Umgebungsvariablen.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub whisper_model: PathBuf,
    pub vad_model: PathBuf,
    pub language: String,
    pub ollama_url: String,
    pub ollama_model: String,
    pub num_ctx: u32,
    /// Adresse der Tuto-Instanz, an die Zusammenfassungen gesendet werden.
    pub tuto_url: String,
    /// Höchstlänge einer Zusammenfassung in Zeichen, wie sie die Tuto-Instanz annimmt.
    pub summary_limit: usize,
    /// Länge der Aufnahme-Segmente. Bestimmt die Genauigkeit von "letzte X min löschen".
    pub segment_secs: u32,
    pub delete_audio_after_transcription: bool,
    /// Nach erfolgreichem Senden an Tuto die lokale Sitzung (Transkripte, Zusammenfassung) behalten. Standard: löschen.
    pub keep_sessions_after_upload: bool,
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
            tuto_url: "https://hackathon.sebastian-bleuel.de".into(),
            summary_limit: 16383,
            segment_secs: 10,
            delete_audio_after_transcription: true,
            keep_sessions_after_upload: false,
            class_list: Vec::new(),
        }
    }
}

impl Config {
    /// Konfigurationsdatei: `TUTO_MIC_CONFIG` oder `<config_dir>/config.json`.
    pub fn path() -> PathBuf {
        std::env::var_os("TUTO_MIC_CONFIG").map_or_else(|| dirs().config_dir().join("config.json"), PathBuf::from)
    }

    /// Standardwerte, darüber die Datei (falls vorhanden), darüber die Umgebungsvariablen. Eine fehlerhafte Datei
    /// oder Variable ist ein Fehler und wird nicht still ersetzt – sonst ginge unbemerkt die Klassenliste und damit
    /// die Pseudonymisierung verloren.
    pub fn load() -> Result<Self> {
        let mut cfg = Self::load_from(&Self::path())?;
        cfg.apply_env(|name| std::env::var(name).ok())?;
        Ok(cfg)
    }

    /// Schreibt die Datei (nur aus dem Admin-Modus). Umgebungsvariablen haben beim nächsten Laden weiter Vorrang.
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        std::fs::create_dir_all(path.parent().expect("Konfigurationspfad liegt in einem Verzeichnis"))?;
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("Konfiguration nicht schreibbar: {}", path.display()))
    }

    /// Überschreibt Werte aus `TUTO_MIC_*`-Variablen (`get` liefert den Wert einer Variable).
    fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) -> Result<()> {
        fn parse<T: std::str::FromStr>(name: &str, value: &str) -> Result<T> {
            value
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("{name}: ungültiger Wert „{value}“"))
        }
        for &(name, _) in ENV_VARS {
            let Some(value) = get(name) else { continue };
            match name {
                "TUTO_MIC_TUTO_URL" => self.tuto_url = value,
                "TUTO_MIC_SUMMARY_LIMIT" => self.summary_limit = parse(name, &value)?,
                "TUTO_MIC_OLLAMA_URL" => self.ollama_url = value,
                "TUTO_MIC_OLLAMA_MODEL" => self.ollama_model = value,
                "TUTO_MIC_LANGUAGE" => self.language = value,
                "TUTO_MIC_WHISPER_MODEL" => self.whisper_model = value.into(),
                "TUTO_MIC_VAD_MODEL" => self.vad_model = value.into(),
                "TUTO_MIC_DELETE_AUDIO" => self.delete_audio_after_transcription = parse(name, &value)?,
                "TUTO_MIC_KEEP_SESSIONS" => self.keep_sessions_after_upload = parse(name, &value)?,
                "TUTO_MIC_CLASS_LIST" => self.class_list = parse_class_list(&value.replace(';', "\n")),
                _ => unreachable!("jede Variable aus ENV_VARS hat einen Zweig"),
            }
        }
        Ok(())
    }

    fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).with_context(|| format!("Konfiguration fehlerhaft: {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("Konfiguration nicht lesbar: {}", path.display())),
        }
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
    fn env_overrides_file_values() {
        let mut cfg = Config::default();
        let env = |name: &str| match name {
            "TUTO_MIC_TUTO_URL" => Some("https://tuto.schule.example".to_string()),
            "TUTO_MIC_SUMMARY_LIMIT" => Some(" 9000 ".to_string()),
            "TUTO_MIC_DELETE_AUDIO" => Some("false".to_string()),
            "TUTO_MIC_CLASS_LIST" => Some("Lena Wagner; Tom Becker".to_string()),
            _ => None,
        };
        cfg.apply_env(env).unwrap();
        assert_eq!(cfg.tuto_url, "https://tuto.schule.example");
        assert_eq!(cfg.summary_limit, 9000);
        assert!(!cfg.delete_audio_after_transcription);
        assert_eq!(cfg.class_list, ["Lena Wagner", "Tom Becker"]);
        assert_eq!(cfg.ollama_model, Config::default().ollama_model);
    }

    #[test]
    fn invalid_env_value_is_an_error() {
        let mut cfg = Config::default();
        let err = cfg
            .apply_env(|name| (name == "TUTO_MIC_SUMMARY_LIMIT").then(|| "viel".to_string()))
            .unwrap_err();
        assert!(err.to_string().contains("TUTO_MIC_SUMMARY_LIMIT"), "{err}");
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
