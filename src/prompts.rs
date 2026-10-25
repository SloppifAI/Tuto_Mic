//! Der Zusammenfassungs-Prompt ist in die App eingebaut (`prompts/summary.md`) und lässt sich im Admin-Modus über
//! den Editor überschreiben. Die eigene Fassung liegt verschlüsselt in `<config_dir>/prompts/summary.md.enc` (bzw.
//! `$TUTO_MIC_PROMPTS`), damit sie niemand unbemerkt ändert oder mitliest.

use anyhow::{Context, Result};
use std::path::PathBuf;

pub const BUILTIN: &str = include_str!("../prompts/summary.md");

/// Stelle im Prompt, an der das Transkript eingesetzt wird. Fehlt sie, wird das Transkript angehängt.
pub const TRANSCRIPT_PLACEHOLDER: &str = "{{TRANSKRIPT}}";

pub fn override_dir() -> PathBuf {
    std::env::var_os("TUTO_MIC_PROMPTS")
        .map_or_else(|| crate::config::dirs().config_dir().join("prompts"), PathBuf::from)
}

pub fn override_path() -> PathBuf {
    override_dir().join("summary.md")
}

pub fn is_overridden() -> bool {
    let path = override_path();
    path.is_file() || crate::crypt::encrypted_path(&path).is_file()
}

/// Überschriebener Prompt, falls vorhanden, sonst der eingebaute. Der eigene Prompt liegt verschlüsselt (Schlüssel
/// im Schlüsselbund); eine alte Klartext-Datei wird dabei umgewandelt. Ist er unlesbar, gilt der eingebaute.
pub fn load() -> String {
    let path = override_path();
    let key = crate::crypt::config_key().ok();
    if let Some(key) = &key
        && let Err(e) = crate::crypt::migrate_protected(&path, key)
    {
        log::warn!("Prompt nicht verschlüsselbar: {e:#}");
    }
    match crate::crypt::read_protected(&path, key.as_ref()) {
        Ok(Some(text)) => text,
        Ok(None) => BUILTIN.to_string(),
        Err(e) => {
            log::warn!("Eigener Prompt nicht lesbar, es gilt der eingebaute: {e:#}");
            BUILTIN.to_string()
        }
    }
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
    let key = crate::crypt::config_key()?;
    crate::crypt::write_protected(&override_path(), &key, text).context("Prompt nicht speicherbar")
}

pub fn reset() -> Result<()> {
    crate::crypt::remove_protected(&override_path()).context("Prompt nicht löschbar")
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
