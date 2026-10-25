//! Ablage des Tuto-Zugangs im Schlüsselbund des Betriebssystems (Windows-Anmeldeinformationsverwaltung,
//! macOS-Schlüsselbund, unter Linux der Secret Service von GNOME/KDE). Nie in `config.json` oder Logs.

use super::Session;
use anyhow::{Context, Result};

const SERVICE: &str = "tuto-mic";
const ENTRY: &str = "tuto-session";

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, ENTRY).context("Schlüsselbund des Betriebssystems nicht verfügbar")
}

pub fn save(session: &Session) -> Result<()> {
    let json = serde_json::to_string(session)?;
    entry()?
        .set_password(&json)
        .context("Zugang konnte nicht im Schlüsselbund gespeichert werden")
}

/// Gespeicherter Zugang, falls vorhanden. Ein unlesbarer Eintrag zählt als „nicht verbunden“.
pub fn load() -> Result<Option<Session>> {
    match entry()?.get_password() {
        Ok(json) => Ok(serde_json::from_str(&json).ok()),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("Schlüsselbund nicht lesbar"),
    }
}

pub fn delete() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("Zugang konnte nicht aus dem Schlüsselbund gelöscht werden"),
    }
}
