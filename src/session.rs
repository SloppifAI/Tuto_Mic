//! Ein Verzeichnis pro Aufnahme bzw. Import.

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};

/// Jede Aufnahme bzw. jeder Import bekommt ein eigenes Verzeichnis:
/// `audio/` (Segmente, nur bei Aufnahmen), `transcript.txt`, `transcript.anon.txt`, `summary.md`, `export.json`,
/// `log.txt`. Aus der GUI alles verschlüsselt mit der zusätzlichen Endung `.enc` (siehe `crypt`).
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

/// Löscht eine Sitzung samt Transkripten und Zusammenfassung. Nur Verzeichnisse unter `sessions_dir()` – ein
/// falscher Pfad darf nie zu einem Löschen anderswo führen.
pub fn delete(dir: &Path) -> Result<()> {
    delete_in(&sessions_dir(), dir)
}

fn delete_in(sessions: &Path, dir: &Path) -> Result<()> {
    anyhow::ensure!(
        dir.parent() == Some(sessions) && dir.file_name().is_some(),
        "Kein Sitzungsverzeichnis: {}",
        dir.display()
    );
    std::fs::remove_dir_all(dir).with_context(|| format!("Sitzung nicht löschbar: {}", dir.display()))
}

/// Sperre auf das Sitzungsverzeichnis, solange eine GUI läuft. Das Betriebssystem gibt sie auch nach einem
/// Absturz frei.
pub struct Lock(#[expect(dead_code, reason = "hält nur die Sperre bis zum Drop")] std::fs::File);

/// Sperrt `sessions_dir()` für diese App-Instanz. `None`, wenn eine andere Instanz es schon gesperrt hat.
pub fn lock() -> Result<Option<Lock>> {
    lock_in(&sessions_dir())
}

fn lock_in(sessions: &Path) -> Result<Option<Lock>> {
    std::fs::create_dir_all(sessions)
        .with_context(|| format!("Sitzungsverzeichnis nicht anlegbar: {}", sessions.display()))?;
    let path = sessions.join(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("Sperrdatei nicht anlegbar: {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Lock(file))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("Sperrdatei nicht sperrbar: {}", path.display()))
        }
    }
}

/// Löscht Sitzungen, die niemand mehr lesen kann: Der Schlüssel einer Sitzung lebt nur im Speicher der App, nach
/// einem Neustart (Beenden oder Absturz) sind ihre verschlüsselten Dateien wertlos. Ebenso weg kommen leere
/// Sitzungen (Aufnahme abgebrochen, bevor ein Segment fertig war). Sitzungen nur mit Klartext-Dateien bleiben
/// (`tuto-mic process`, Entwicklerwerkzeug). Nur aufrufen, wenn [`lock`] die Sperre bekommen hat – sonst gehören
/// die Sitzungen womöglich einer laufenden Instanz. Liefert die Zahl der gelöschten Sitzungen.
pub fn remove_unreadable() -> Result<usize> {
    remove_unreadable_in(&sessions_dir())
}

fn remove_unreadable_in(sessions: &Path) -> Result<usize> {
    let entries = match std::fs::read_dir(sessions) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e).with_context(|| format!("Sitzungen nicht lesbar: {}", sessions.display())),
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        // `file_type` folgt keinen symbolischen Links: Nur echte Verzeichnisse hier drin zählen.
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let dir = entry.path();
        // Eine Sitzung, die sich nicht prüfen oder löschen lässt, hält die anderen nicht auf. Ohne Pfad im Log:
        // Der Name einer importierten Sitzung stammt vom Dateinamen und kann Namen enthalten.
        match is_unreadable(&dir).and_then(|unreadable| {
            if unreadable {
                delete_in(sessions, &dir)?;
            }
            Ok(unreadable)
        }) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(e) => log::warn!("Alte Sitzung nicht aufräumbar: {}", e.root_cause()),
        }
    }
    Ok(removed)
}

/// Ohne Dateien oder mit mindestens einer verschlüsselten Datei (auch in `audio/`).
fn is_unreadable(dir: &Path) -> Result<bool> {
    let (files, encrypted) = scan(dir)?;
    Ok(files == 0 || encrypted)
}

/// Zahl der Dateien unter `dir` und ob eine davon verschlüsselt ist.
fn scan(dir: &Path) -> Result<(usize, bool)> {
    let (mut files, mut encrypted) = (0, false);
    for entry in std::fs::read_dir(dir).with_context(|| format!("Sitzung nicht lesbar: {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            let (f, e) = scan(&path)?;
            files += f;
            encrypted |= e;
        } else {
            files += 1;
            encrypted |= crate::crypt::is_encrypted_file(&path)?;
        }
    }
    Ok((files, encrypted))
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
    fn deletes_only_session_directories() {
        let sessions = tempfile::tempdir().unwrap();
        let session = sessions.path().join("2026-10-25_08-05-00");
        std::fs::create_dir_all(session.join("audio")).unwrap();
        std::fs::write(session.join("summary.md"), "x").unwrap();
        delete_in(sessions.path(), &session).unwrap();
        assert!(!session.exists());

        // Alles außerhalb des Sitzungsordners wird verweigert.
        assert!(delete_in(sessions.path(), sessions.path()).is_err());
        assert!(delete_in(sessions.path(), Path::new("/tmp")).is_err());
        assert!(sessions.path().exists());
    }

    #[test]
    fn removes_unreadable_sessions_only() {
        use crate::crypt::{self, Key};
        let sessions = tempfile::tempdir().unwrap();
        let key = Key::generate().unwrap();
        let mk = |name: &str| {
            let dir = sessions.path().join(name);
            std::fs::create_dir_all(audio_dir(&dir)).unwrap();
            dir
        };
        // Aufnahme nach Absturz: nur verschlüsselte Segmente.
        let crashed = mk("2026-10-25_08-00-00");
        crypt::write(&audio_dir(&crashed).join("seg_00001.wav.enc"), &key, b"RIFF").unwrap();
        // Fertig ausgewertet, aber nicht gesendet.
        let finished = mk("2026-10-25_09-00-00");
        crypt::write(&finished.join("summary.md.enc"), &key, b"# Mathe").unwrap();
        // Aufnahme ohne fertiges Segment: leer.
        let empty = mk("2026-10-25_10-00-00");
        // Ergebnis von `tuto-mic process` im Klartext bleibt, auch mit leerem Unterverzeichnis.
        let cli = mk("2026-10-25_11-00-00_stunde");
        std::fs::write(cli.join("summary.md"), "# Mathe").unwrap();
        std::fs::write(sessions.path().join(".lock"), "").unwrap();

        assert_eq!(remove_unreadable_in(sessions.path()).unwrap(), 3);
        for dir in [&crashed, &finished, &empty] {
            assert!(!dir.exists(), "{}", dir.display());
        }
        assert!(cli.join("summary.md").exists());
        assert!(sessions.path().join(".lock").exists());
        assert_eq!(remove_unreadable_in(sessions.path()).unwrap(), 0);
        assert_eq!(remove_unreadable_in(&sessions.path().join("fehlt")).unwrap(), 0);
    }

    #[test]
    fn lock_is_exclusive() {
        let sessions = tempfile::tempdir().unwrap();
        let first = lock_in(sessions.path()).unwrap();
        assert!(first.is_some());
        assert!(lock_in(sessions.path()).unwrap().is_none());
        drop(first);
        assert!(lock_in(sessions.path()).unwrap().is_some());
    }

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
