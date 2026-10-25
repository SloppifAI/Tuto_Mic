//! Der Zusammenfassungs-Prompt ist in die App eingebaut (`prompts/summary.md`) und lässt sich zum Testen
//! überschreiben: im Dev-Modus über den Editor, oder per Datei `summary.md` in `<config_dir>/prompts/`
//! bzw. `$TUTO_MIC_PROMPTS`.

use anyhow::Result;
use std::path::PathBuf;

pub const BUILTIN: &str = include_str!("../prompts/summary.md");

pub fn override_dir() -> PathBuf {
    std::env::var_os("TUTO_MIC_PROMPTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::dirs().config_dir().join("prompts"))
}

fn override_path() -> PathBuf {
    override_dir().join("summary.md")
}

pub fn is_overridden() -> bool {
    override_path().is_file()
}

/// Überschriebener Prompt, falls vorhanden, sonst der eingebaute.
pub fn load() -> String {
    std::fs::read_to_string(override_path()).unwrap_or_else(|_| BUILTIN.to_string())
}

pub fn save(text: &str) -> Result<()> {
    std::fs::create_dir_all(override_dir())?;
    std::fs::write(override_path(), text)?;
    Ok(())
}

pub fn reset() -> Result<()> {
    let path = override_path();
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}
