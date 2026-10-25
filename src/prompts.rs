//! Der Zusammenfassungs-Prompt ist in die App eingebaut (`prompts/summary.md`) und lässt sich zum Testen
//! überschreiben: im Dev-Modus über den Editor, oder per Datei `summary.md` in `<config_dir>/prompts/`
//! bzw. `$TUTO_MIC_PROMPTS`.

use anyhow::{Context, Result};
use std::path::PathBuf;

pub const BUILTIN: &str = include_str!("../prompts/summary.md");

/// Stelle im Prompt, an der das Transkript eingesetzt wird. Fehlt sie, wird das Transkript angehängt.
pub const TRANSCRIPT_PLACEHOLDER: &str = "{{TRANSKRIPT}}";

pub fn override_dir() -> PathBuf {
    std::env::var_os("TUTO_MIC_PROMPTS")
        .map_or_else(|| crate::config::dirs().config_dir().join("prompts"), PathBuf::from)
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

/// Setzt das Transkript in den Prompt ein.
pub fn render(prompt: &str, transcript: &str) -> String {
    if prompt.contains(TRANSCRIPT_PLACEHOLDER) {
        prompt.replace(TRANSCRIPT_PLACEHOLDER, transcript.trim_end())
    } else {
        format!("{prompt}{transcript}")
    }
}

pub fn save(text: &str) -> Result<()> {
    let path = override_path();
    std::fs::create_dir_all(override_dir())
        .and_then(|()| std::fs::write(&path, text))
        .with_context(|| format!("Prompt nicht speicherbar: {}", path.display()))?;
    Ok(())
}

pub fn reset() -> Result<()> {
    let path = override_path();
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("Prompt nicht löschbar: {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_prompt_has_placeholder() {
        assert_eq!(BUILTIN.matches(TRANSCRIPT_PLACEHOLDER).count(), 1);
    }

    #[test]
    fn render_inserts_transcript_at_placeholder() {
        let out = render("Vorher\n<t>\n{{TRANSKRIPT}}\n</t>\n", "[00:01] Hallo\n");
        assert_eq!(out, "Vorher\n<t>\n[00:01] Hallo\n</t>\n");
    }

    #[test]
    fn render_appends_without_placeholder() {
        assert_eq!(render("Prompt:\n", "[00:01] Hallo\n"), "Prompt:\n[00:01] Hallo\n");
    }
}
