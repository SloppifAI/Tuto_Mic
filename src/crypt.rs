//! Verschlüsselung der Sitzungsdaten auf der Festplatte (Aufnahme-Segmente, Transkripte, Zusammenfassung,
//! Export, Log).
//!
//! Jede Aufnahme bzw. Auswertung in der GUI bekommt einen zufälligen [`Key`], der nur im Arbeitsspeicher liegt
//! und nirgends gespeichert wird. Endet die App (regulär oder durch Absturz), ist der Schlüssel weg und die
//! Dateien sind wertlos; `session::remove_unreadable` räumt sie beim nächsten Start ab. So liegt auf dem
//! gemeinsam genutzten Schulrechner nie etwas Personenbezogenes im Klartext.
//!
//! Dateiformat (Version 1), alle Längen in Byte:
//!
//! | Offset | Länge | Inhalt                                                        |
//! |--------|-------|---------------------------------------------------------------|
//! | 0      | 4     | Kennung `TMIC`                                                |
//! | 4      | 1     | Formatversion `1`                                             |
//! | 5      | 24    | Nonce, für jede Datei neu zufällig                            |
//! | 29     | n+16  | Ciphertext mit angehängtem Poly1305-Tag (16 Byte)             |
//!
//! Verfahren: XChaCha20-Poly1305 (AEAD). Die 24-Byte-Nonce ist groß genug, um sie zufällig zu wählen, ohne
//! Wiederholungen befürchten zu müssen. Kennung und Version gehen als zusätzliche Daten (AAD) in den Tag ein,
//! eine Änderung am Kopf fällt also ebenso auf wie eine am Inhalt. Verschlüsselte Dateien tragen die
//! zusätzliche Endung [`EXTENSION`] (`summary.md.enc`).

use anyhow::{Context, Result, bail, ensure};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

/// Zusätzliche Dateiendung verschlüsselter Dateien.
pub const EXTENSION: &str = "enc";

const MAGIC: &[u8; 4] = b"TMIC";
const VERSION: u8 = 1;
const HEADER_LEN: usize = MAGIC.len() + 1;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const KEY_LEN: usize = 32;

/// Schlüssel einer Sitzung. Liegt nur im Arbeitsspeicher und wird beim Freigeben überschrieben.
///
/// Im `Box`, damit Verschieben nur den Zeiger kopiert und keine Kopien des Schlüssels auf dem Stack zurücklässt.
/// `Clone` ist gewollt (Aufnahme-Thread und Auswertung brauchen ihn); jede Kopie wird für sich überschrieben.
#[derive(Clone)]
pub struct Key(Box<[u8; KEY_LEN]>);

impl Key {
    /// Neuer zufälliger Schlüssel aus dem Zufallsgenerator des Betriebssystems.
    pub fn generate() -> Result<Self> {
        let mut key = Box::new([0u8; KEY_LEN]);
        getrandom::fill(key.as_mut_slice())
            .map_err(|e| anyhow::anyhow!("Kein Zufall vom Betriebssystem für den Schlüssel: {e}"))?;
        Ok(Self(key))
    }

    /// Schlüssel aus gespeicherten Bytes (z. B. aus dem Schlüsselbund).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let array: [u8; KEY_LEN] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Schlüssel hat die falsche Länge"))?;
        Ok(Self(Box::new(array)))
    }

    /// Die Schlüssel-Bytes, nur um sie im Schlüsselbund abzulegen.
    fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(<&chacha20poly1305::Key>::from(&*self.0))
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Nie den Schlüssel ausgeben, auch nicht versehentlich in einem Log.
impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(…)")
    }
}

fn header() -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..MAGIC.len()].copy_from_slice(MAGIC);
    h[MAGIC.len()] = VERSION;
    h
}

/// Verschlüsselt `plaintext` im obigen Format, mit frischer Nonce.
pub fn encrypt(key: &Key, plaintext: &[u8]) -> Result<Vec<u8>> {
    let header = header();
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("Kein Zufall vom Betriebssystem für die Nonce: {e}"))?;
    let ciphertext = key
        .cipher()
        .encrypt(
            <&XNonce>::from(&nonce),
            Payload {
                msg: plaintext,
                aad: &header,
            },
        )
        .map_err(|_| anyhow::anyhow!("Verschlüsselung fehlgeschlagen"))?;
    let mut out = Vec::with_capacity(HEADER_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Entschlüsselt Daten im obigen Format. Schlägt fehl bei falschem Schlüssel, veränderten oder unvollständigen
/// Daten.
pub fn decrypt(key: &Key, data: &[u8]) -> Result<Vec<u8>> {
    ensure!(is_encrypted(data), "Keine verschlüsselte Tuto-Mic-Datei");
    let version = data[MAGIC.len()];
    ensure!(version == VERSION, "Unbekannte Version {version} der Verschlüsselung");
    if data.len() < HEADER_LEN + NONCE_LEN + TAG_LEN {
        bail!("Verschlüsselte Datei ist unvollständig");
    }
    let (header, rest) = data.split_at(HEADER_LEN);
    let (nonce, ciphertext) = rest.split_at(NONCE_LEN);
    let nonce = XNonce::slice_as_array(nonce).context("Nonce hat die falsche Länge")?;
    key.cipher()
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad: header,
            },
        )
        .map_err(|_| anyhow::anyhow!("Entschlüsselung fehlgeschlagen: falscher Schlüssel oder Datei beschädigt"))
}

/// Dauerhafter Schlüssel für die Konfiguration (Klassenliste, Prompt, Einstellungen). Liegt im Schlüsselbund des
/// Betriebssystems, den die Anmeldung am Rechner entsperrt; wird beim ersten Bedarf zufällig erzeugt. Anders als
/// der Sitzungsschlüssel überdauert er Neustarts.
pub fn config_key() -> Result<Key> {
    static CACHE: std::sync::OnceLock<Result<Key, String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| load_or_create_config_key().map_err(|e| format!("{e:#}")))
        .clone()
        .map_err(|e| anyhow::anyhow!(e))
}

fn load_or_create_config_key() -> Result<Key> {
    let entry =
        keyring::Entry::new("tuto-mic", "config-key").context("Schlüsselbund des Betriebssystems nicht verfügbar")?;
    match entry.get_secret() {
        Ok(bytes) => Key::from_bytes(&bytes),
        Err(keyring::Error::NoEntry) => {
            let key = Key::generate()?;
            entry
                .set_secret(key.as_bytes())
                .context("Schlüssel für die Konfiguration nicht im Schlüsselbund speicherbar")?;
            Ok(key)
        }
        Err(e) => Err(e).context("Schlüsselbund nicht lesbar"),
    }
}

/// Liest eine geschützte Textdatei: bevorzugt `<path>.enc`, sonst eine alte Klartext-Datei `<path>`. `None`, wenn
/// keine existiert. Ohne Schlüssel (Schlüsselbund fehlt) ist nur Klartext lesbar.
pub fn read_protected(path: &Path, key: Option<&Key>) -> Result<Option<String>> {
    let enc = encrypted_path(path);
    if enc.exists() {
        let key = key.context(
            "Konfiguration ist verschlüsselt, aber der Schlüsselbund des Betriebssystems ist nicht verfügbar",
        )?;
        let bytes = read(&enc, key)?;
        return String::from_utf8(bytes)
            .map(Some)
            .context("Entschlüsselte Datei ist kein Text");
    }
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Datei nicht lesbar: {}", path.display())),
    }
}

/// Schreibt `text` verschlüsselt nach `<path>.enc` und entfernt eine alte Klartext-Datei `<path>`.
pub fn write_protected(path: &Path, key: &Key, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("Verzeichnis nicht anlegbar: {}", dir.display()))?;
    }
    write(&encrypted_path(path), key, text.as_bytes())?;
    remove_if_exists(path)
}

/// Entfernt `<path>` und `<path>.enc`.
pub fn remove_protected(path: &Path) -> Result<()> {
    remove_if_exists(path)?;
    remove_if_exists(&encrypted_path(path))
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("Datei nicht löschbar: {}", path.display())),
    }
}

/// Wandelt eine alte Klartext-Datei `<path>` in `<path>.enc` um. `true`, wenn umgewandelt wurde.
pub fn migrate_protected(path: &Path, key: &Key) -> Result<bool> {
    if encrypted_path(path).exists() || !path.exists() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("Datei nicht lesbar: {}", path.display()))?;
    write_protected(path, key, &text)?;
    Ok(true)
}

/// Ob `data` mit der Kennung verschlüsselter Dateien beginnt (Version egal).
pub fn is_encrypted(data: &[u8]) -> bool {
    data.len() > MAGIC.len() && data.starts_with(MAGIC)
}

/// Ob die Datei unter `path` mit der Kennung beginnt. Liest nur den Kopf.
pub fn is_encrypted_file(path: &Path) -> Result<bool> {
    use std::io::Read;
    let mut head = Vec::with_capacity(HEADER_LEN);
    std::fs::File::open(path)
        .and_then(|f| f.take(HEADER_LEN as u64).read_to_end(&mut head))
        .with_context(|| format!("Datei nicht lesbar: {}", path.display()))?;
    Ok(is_encrypted(&head))
}

/// `path` mit angehängter Endung [`EXTENSION`].
pub fn encrypted_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".");
    name.push(EXTENSION);
    PathBuf::from(name)
}

/// Schreibt `plaintext` verschlüsselt nach `path` (den Namen mit [`encrypted_path`] bilden).
pub fn write(path: &Path, key: &Key, plaintext: &[u8]) -> Result<()> {
    let data = encrypt(key, plaintext)?;
    std::fs::write(path, data).with_context(|| format!("Datei nicht schreibbar: {}", path.display()))
}

/// Liest und entschlüsselt die Datei unter `path`.
pub fn read(path: &Path, key: &Key) -> Result<Vec<u8>> {
    let data = std::fs::read(path).with_context(|| format!("Datei nicht lesbar: {}", path.display()))?;
    decrypt(key, &data).with_context(|| format!("Datei nicht entschlüsselbar: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &[u8] = b"Lena Wagner rechnet 25 % vor.";

    #[test]
    fn roundtrip() {
        let key = Key::generate().unwrap();
        let data = encrypt(&key, TEXT).unwrap();
        assert_eq!(data.len(), HEADER_LEN + NONCE_LEN + TEXT.len() + TAG_LEN);
        assert!(data.starts_with(b"TMIC\x01"));
        assert!(!data.windows(4).any(|w| w == b"Lena"), "Klartext in der Datei");
        assert_eq!(decrypt(&key, &data).unwrap(), TEXT);
        // Auch leerer Inhalt ist gültig.
        assert_eq!(decrypt(&key, &encrypt(&key, b"").unwrap()).unwrap(), b"");
    }

    #[test]
    fn fresh_nonce_per_encryption() {
        let key = Key::generate().unwrap();
        let a = encrypt(&key, TEXT).unwrap();
        let b = encrypt(&key, TEXT).unwrap();
        assert_ne!(
            a[HEADER_LEN..HEADER_LEN + NONCE_LEN],
            b[HEADER_LEN..HEADER_LEN + NONCE_LEN]
        );
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_key_fails() {
        let data = encrypt(&Key::generate().unwrap(), TEXT).unwrap();
        let err = decrypt(&Key::generate().unwrap(), &data).unwrap_err();
        assert!(format!("{err:#}").contains("falscher Schlüssel"), "{err:#}");
    }

    #[test]
    fn tampering_is_detected() {
        let key = Key::generate().unwrap();
        let data = encrypt(&key, TEXT).unwrap();
        // Jedes einzelne Byte: Kopf, Nonce, Ciphertext und Tag.
        for i in 0..data.len() {
            let mut bad = data.clone();
            bad[i] ^= 0x01;
            assert!(decrypt(&key, &bad).is_err(), "Änderung an Byte {i} nicht erkannt");
        }
        let mut longer = data.clone();
        longer.push(0);
        assert!(decrypt(&key, &longer).is_err());
    }

    #[test]
    fn truncated_data_fails() {
        let key = Key::generate().unwrap();
        let data = encrypt(&key, TEXT).unwrap();
        for len in 0..data.len() {
            assert!(decrypt(&key, &data[..len]).is_err(), "gekürzt auf {len} Byte");
        }
    }

    #[test]
    fn plaintext_is_not_accepted() {
        let key = Key::generate().unwrap();
        assert!(!is_encrypted(TEXT));
        assert!(!is_encrypted(b"TMIC"));
        let err = decrypt(&key, TEXT).unwrap_err();
        assert!(format!("{err:#}").contains("Keine verschlüsselte"), "{err:#}");
    }

    #[test]
    fn files_roundtrip_and_detection() {
        let dir = tempfile::tempdir().unwrap();
        let key = Key::generate().unwrap();
        let path = encrypted_path(&dir.path().join("summary.md"));
        assert_eq!(path.file_name().unwrap(), "summary.md.enc");
        write(&path, &key, TEXT).unwrap();
        assert!(is_encrypted_file(&path).unwrap());
        assert_eq!(read(&path, &key).unwrap(), TEXT);
        assert!(read(&path, &Key::generate().unwrap()).is_err());

        let plain = dir.path().join("plain.txt");
        std::fs::write(&plain, TEXT).unwrap();
        assert!(!is_encrypted_file(&plain).unwrap());
        let empty = dir.path().join("leer");
        std::fs::write(&empty, b"").unwrap();
        assert!(!is_encrypted_file(&empty).unwrap());
    }

    #[test]
    fn clone_is_independent_and_debug_hides_key() {
        let key = Key::generate().unwrap();
        let copy = key.clone();
        let data = encrypt(&key, TEXT).unwrap();
        drop(key);
        assert_eq!(decrypt(&copy, &data).unwrap(), TEXT);
        assert_eq!(format!("{copy:?}"), "Key(…)");
    }
}
