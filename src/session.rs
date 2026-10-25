//! Ein Verzeichnis pro Aufnahme bzw. Import.

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};

/// Jede Aufnahme bzw. jeder Import bekommt ein eigenes Verzeichnis:
/// `audio/` (Segmente, nur bei Aufnahmen), `transcript.txt`, `transcript.anon.txt`, `summary.md`, `export.json`,
/// `log.txt`.
pub fn sessions_dir() -> PathBuf {
    crate::config::dirs().data_dir().join("sessions")
}

/// Verzeichnis für eine Aufnahme, mit `audio/` für die Segmente des Recorders.
pub fn create_recording() -> Result<PathBuf> {
    let dir = create_in(&sessions_dir(), None)?;
    let audio = audio_dir(&dir);
    std::fs::create_dir_all(&audio)
        .with_context(|| format!("Audio-Verzeichnis nicht anlegbar: {}", audio.display()))?;
    Ok(dir)
}

/// Verzeichnis für einen Import (Audiodatei oder Transkript) unterhalb von `sessions`. Ohne `audio/`: Es bliebe
/// leer, weil importierte Dateien nicht kopiert werden.
pub fn create_in(sessions: &Path, label: Option<&str>) -> Result<PathBuf> {
    let dir = sessions.join(dir_name(Local::now(), label));
    std::fs::create_dir_all(&dir).with_context(|| format!("Aufnahme-Verzeichnis nicht anlegbar: {}", dir.display()))?;
    Ok(dir)
}

pub fn audio_dir(session: &Path) -> PathBuf {
    session.join("audio")
}

/// Zeitstempel plus optionaler Bezeichnung, auf dateisystemtaugliche Zeichen reduziert.
fn dir_name(now: DateTime<Local>, label: Option<&str>) -> String {
    let stamp = now.format("%Y-%m-%d_%H-%M-%S");
    match label {
        Some(label) => {
            let label: String = label
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
                .collect();
            format!("{stamp}_{label}")
        }
        None => stamp.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn dir_name_is_filesystem_safe() {
        let now = Local.with_ymd_and_hms(2026, 10, 25, 8, 5, 0).unwrap();
        assert_eq!(dir_name(now, None), "2026-10-25_08-05-00");
        assert_eq!(
            dir_name(now, Some("Mathe 7b/Prozente")),
            "2026-10-25_08-05-00_Mathe_7b_Prozente"
        );
        assert_eq!(dir_name(now, Some("Übung-2")), "2026-10-25_08-05-00_Übung-2");
    }

    #[test]
    fn import_dir_has_no_audio_dir() {
        let base = tempfile::tempdir().unwrap();
        let dir = create_in(base.path(), Some("import")).unwrap();
        assert!(dir.is_dir());
        assert!(!audio_dir(&dir).exists());
    }
}
