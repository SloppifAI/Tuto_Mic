use anyhow::Result;
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};

/// Jede Aufnahme bzw. jeder Import bekommt ein eigenes Verzeichnis:
/// `audio/` (Segmente), `transcript.txt`, `transcript.anon.txt`, `summary.md`, `export.json`, `log.txt`.
pub fn sessions_dir() -> PathBuf {
    crate::config::dirs().data_dir().join("sessions")
}

pub fn create(label: Option<&str>) -> Result<PathBuf> {
    let dir = sessions_dir().join(dir_name(Local::now(), label));
    std::fs::create_dir_all(audio_dir(&dir))?;
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
}
