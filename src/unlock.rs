//! Passwort für die verschlüsselte Konfiguration (Einstellungen, Klassenliste, eigener Prompt).
//!
//! Der Schlüssel wird bei jedem Start mit Argon2id aus dem Passwort und einem zufälligen Salt abgeleitet und lebt
//! nur im Arbeitsspeicher. Ohne Passwort ist nichts davon lesbar, auch nicht im selben Benutzerkonto. Gespeichert
//! wird nur das Salt (nicht geheim, `config.salt`); ob das Passwort stimmt, zeigt sich daran, dass sich
//! `config.json.enc` entschlüsseln lässt. Ein vergessenes Passwort lässt sich in der App bewusst nicht zurücksetzen –
//! sonst könnte jeder ohne Passwort ein eigenes setzen und die App übernehmen. Das macht die Betreuung, indem sie
//! die Dateien löscht (siehe docs/installation.md).

use crate::config::Config;
use crate::crypt::{self, Key};
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Mindestlänge, damit Argon2 nicht gegen ein triviales Passwort arbeiten muss.
pub const MIN_LEN: usize = 8;
const SALT_LEN: usize = 16;

static UNLOCKED: OnceLock<Key> = OnceLock::new();

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    /// Noch kein Passwort festgelegt (erster Start).
    NeedsSetup,
    /// Passwort festgelegt, App noch gesperrt.
    Locked,
    /// In diesem Prozess entsperrt.
    Unlocked,
}

/// Dateien, um die es geht. Eigene Struktur, damit Tests ein temporäres Verzeichnis nutzen können.
#[derive(Clone)]
pub struct Paths {
    pub salt: PathBuf,
    pub config: PathBuf,
    pub prompt: PathBuf,
}

impl Paths {
    pub fn current() -> Self {
        let config = Config::path();
        Self {
            salt: config.with_file_name("config.salt"),
            config,
            prompt: crate::prompts::override_path(),
        }
    }
}

pub fn status() -> Status {
    if UNLOCKED.get().is_some() {
        Status::Unlocked
    } else if Paths::current().salt.is_file() {
        Status::Locked
    } else {
        Status::NeedsSetup
    }
}

/// Der Schlüssel dieses Prozesses, sobald entsperrt.
pub fn key() -> Result<Key> {
    UNLOCKED
        .get()
        .cloned()
        .context("Tuto Mic ist gesperrt – erst mit dem Passwort entsperren")
}

/// Erster Start: Passwort festlegen, vorhandene Konfiguration übernehmen (Klartext oder mit dem früheren
/// Schlüsselbund-Schlüssel verschlüsselt) und mit dem neuen Schlüssel speichern.
pub fn setup(password: &str) -> Result<()> {
    let key = setup_in(&Paths::current(), password, crypt::legacy_config_key().as_ref())?;
    crypt::forget_legacy_config_key();
    remember(key);
    Ok(())
}

/// Entsperren mit dem Passwort.
pub fn unlock(password: &str) -> Result<()> {
    remember(unlock_in(&Paths::current(), password)?);
    Ok(())
}

fn remember(key: Key) {
    // Ein zweites Entsperren im selben Prozess hat denselben Schlüssel; der erste bleibt gültig.
    let _ = UNLOCKED.set(key);
}

fn derive(password: &str, salt: &[u8]) -> Result<Key> {
    let mut out = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| anyhow::anyhow!("Schlüssel nicht ableitbar: {e}"))?;
    let key = Key::from_bytes(&out);
    zeroize::Zeroize::zeroize(&mut out);
    key
}

fn setup_in(paths: &Paths, password: &str, legacy: Option<&Key>) -> Result<Key> {
    ensure!(
        password.chars().count() >= MIN_LEN,
        "Das Passwort braucht mindestens {MIN_LEN} Zeichen"
    );
    if paths.salt.exists() {
        bail!("Es ist schon ein Passwort festgelegt");
    }
    // Vorhandenes lesen, bevor etwas überschrieben wird: verschlüsselt mit dem alten Schlüssel oder Klartext.
    let config = read_existing(&paths.config, legacy).context("Bisherige Konfiguration nicht lesbar")?;
    let prompt = read_existing(&paths.prompt, legacy).context("Bisheriger Prompt nicht lesbar")?;

    let mut salt = [0u8; SALT_LEN];
    getrandom::fill(&mut salt).map_err(|e| anyhow::anyhow!("Kein Zufall vom Betriebssystem: {e}"))?;
    let key = derive(password, &salt)?;

    // Die Konfiguration dient zugleich als Prüfung des Passworts, daher immer schreiben (notfalls Standardwerte).
    let config = match config {
        Some(text) => text,
        None => serde_json::to_string_pretty(&Config::default())?,
    };
    crypt::write_protected(&paths.config, &key, &config)?;
    if let Some(prompt) = prompt {
        crypt::write_protected(&paths.prompt, &key, &prompt)?;
    }
    write_salt(&paths.salt, &salt)?;
    Ok(key)
}

fn read_existing(path: &Path, legacy: Option<&Key>) -> Result<Option<String>> {
    crypt::read_protected(path, legacy)
}

fn write_salt(path: &Path, salt: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("Verzeichnis nicht anlegbar: {}", dir.display()))?;
    }
    std::fs::write(path, salt).with_context(|| format!("Datei nicht schreibbar: {}", path.display()))
}

fn unlock_in(paths: &Paths, password: &str) -> Result<Key> {
    let salt = std::fs::read(&paths.salt).context("Noch kein Passwort festgelegt")?;
    let key = derive(password, &salt)?;
    let enc = crypt::encrypted_path(&paths.config);
    let data = std::fs::read(&enc).with_context(|| format!("Konfiguration fehlt: {}", enc.display()))?;
    if crypt::decrypt(&key, &data).is_err() {
        bail!("Passwort falsch");
    }
    Ok(key)
}

/// Was die Betreuung von Hand tut (Dateien löschen), hier nur für Tests.
#[cfg(test)]
fn reset_in(paths: &Paths) -> Result<()> {
    crypt::remove_protected(&paths.config)?;
    crypt::remove_protected(&paths.prompt)?;
    match std::fs::remove_file(&paths.salt) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("Passwort nicht zurücksetzbar"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &Path) -> Paths {
        Paths {
            salt: dir.join("config.salt"),
            config: dir.join("config.json"),
            prompt: dir.join("prompts/summary.md"),
        }
    }

    #[test]
    fn setup_then_unlock() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        let key = setup_in(&p, "richtig-lang", None).unwrap();
        assert!(p.salt.is_file());
        // Konfiguration ist mit dem Passwort-Schlüssel lesbar.
        assert!(crypt::read_protected(&p.config, Some(&key)).unwrap().is_some());

        assert!(unlock_in(&p, "richtig-lang").is_ok());
        let err = unlock_in(&p, "falsches-passwort").unwrap_err();
        assert_eq!(err.to_string(), "Passwort falsch");
        assert!(setup_in(&p, "noch-eins-lang", None).is_err(), "nicht zweimal festlegen");
    }

    #[test]
    fn short_password_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(setup_in(&paths(dir.path()), "kurz", None).is_err());
    }

    #[test]
    fn takes_over_plaintext_and_keychain_encrypted_data() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        // Konfiguration mit dem früheren Schlüsselbund-Schlüssel, Prompt noch im Klartext.
        let legacy = Key::generate().unwrap();
        crypt::write_protected(&p.config, &legacy, r#"{"class_list": ["Lena Wagner"]}"#).unwrap();
        std::fs::create_dir_all(p.prompt.parent().unwrap()).unwrap();
        std::fs::write(&p.prompt, "Eigener Prompt").unwrap();

        let key = setup_in(&p, "richtig-lang", Some(&legacy)).unwrap();
        let config = crypt::read_protected(&p.config, Some(&key)).unwrap().unwrap();
        assert!(config.contains("Lena Wagner"));
        assert_eq!(
            crypt::read_protected(&p.prompt, Some(&key)).unwrap().unwrap(),
            "Eigener Prompt"
        );
        assert!(!p.prompt.exists(), "kein Klartext mehr");
        assert!(
            crypt::read_protected(&p.config, Some(&legacy)).is_err(),
            "alter Schlüssel passt nicht mehr"
        );
    }

    #[test]
    fn reset_removes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        setup_in(&p, "richtig-lang", None).unwrap();
        reset_in(&p).unwrap();
        assert!(!p.salt.exists());
        assert!(!crypt::encrypted_path(&p.config).exists());
        assert!(setup_in(&p, "neues-passwort", None).is_ok());
    }
}
