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
    crate::crypt::encrypted_path(&override_path()).is_file()
}

/// Überschriebener Prompt, falls vorhanden, sonst der eingebaute. Ist er unlesbar, gilt der eingebaute.
pub fn load() -> String {
    load_from(&override_path(), crate::crypt::config_key().ok().as_ref())
}

/// Nur die verschlüsselte Fassung zählt. Eine Klartext-Datei übernimmt einmalig das Festlegen des Passworts
/// (`unlock::setup`); danach würde sie sonst jemand ohne Passwort unbemerkt unterschieben können.
fn load_from(path: &std::path::Path, key: Option<&crate::crypt::Key>) -> String {
    if !crate::crypt::encrypted_path(path).is_file() {
        if path.is_file() {
            log::warn!("Unverschlüsselter Prompt wird ignoriert: {}", path.display());
        }
        return BUILTIN.to_string();
    }
    match crate::crypt::read_protected(path, key) {
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
    fn only_the_encrypted_prompt_counts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("summary.md");
        let key = crate::crypt::Key::generate().unwrap();
        std::fs::write(&path, "Untergeschoben").unwrap();
        assert_eq!(load_from(&path, Some(&key)), BUILTIN);
        assert!(path.exists(), "nicht mit dem echten Schlüssel verschlüsseln");

        crate::crypt::write_protected(&path, &key, "Eigener Prompt").unwrap();
        assert_eq!(load_from(&path, Some(&key)), "Eigener Prompt");
    }

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
