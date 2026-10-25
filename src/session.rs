use anyhow::Result;
use std::path::PathBuf;

/// Jede Aufnahme bzw. jeder Import bekommt ein eigenes Verzeichnis:
/// `audio/` (Segmente), `transcript.txt`, `transcript.anon.txt`, `summary.md`, `export.json`, `log.txt`.
pub fn sessions_dir() -> PathBuf {
    crate::config::dirs().data_dir().join("sessions")
}

pub fn create(label: Option<&str>) -> Result<PathBuf> {
    let mut name = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    if let Some(label) = label {
        let label: String = label
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
            .collect();
        name = format!("{name}_{label}");
    }
    let dir = sessions_dir().join(name);
    std::fs::create_dir_all(dir.join("audio"))?;
    Ok(dir)
}

pub fn audio_dir(session: &std::path::Path) -> PathBuf {
    session.join("audio")
}
