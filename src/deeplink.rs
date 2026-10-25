//! `tutoapp://`-Links: Tuto leitet nach „Erlauben“ auf `tutoapp://callback?code=…` zurück.
//!
//! Damit der Browser diese Adresse an Tuto Mic gibt, meldet sich die App beim Betriebssystem als Programm
//! für `tutoapp://` an ([`register`]). Der Browser startet dann eine *zweite* Instanz mit dem Link als
//! Argument; diese reicht ihn über eine lokale Verbindung an die laufende App weiter ([`forward`]) und
//! beendet sich. Die laufende App nimmt ihn mit [`listen`] entgegen.
//!
//! Sicherheit: Andere Programme könnten sich ebenfalls für `tutoapp://` anmelden oder an den lokalen Port
//! senden. Das schadet nicht, weil der Code ohne den geheimen PKCE-Verifier der laufenden App wertlos ist
//! und `state` geprüft wird.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;

pub const SCHEME: &str = "tutoapp";
/// Fester lokaler Port für die Weitergabe zwischen zwei Instanzen.
const PORT: u16 = 47_825;
/// Längere Zeilen sind kein Rücksprung von Tuto.
const MAX_LINK_LEN: usize = 4096;

pub fn is_link(arg: &str) -> bool {
    arg.starts_with("tutoapp:")
}

/// Nachricht einer zweiten Instanz: „Fenster nach vorne holen“ (Tuto Mic läuft nur einmal).
pub const ACTIVATE: &str = "tutomic:activate";

/// Läuft Tuto Mic schon, wird es nach vorne geholt und `true` geliefert – dann soll dieser Start enden.
pub fn activate_running() -> bool {
    forward(ACTIVATE)
}

/// Meldet Tuto Mic als Programm für `tutoapp://` an. Kann bei jedem Start aufgerufen werden.
pub fn register() -> Result<()> {
    let exe = std::env::current_exe().context("Pfad der App nicht ermittelbar")?;
    platform::register(&exe)
}

/// Gibt `link` an eine bereits laufende App weiter. `true`, wenn sie ihn angenommen hat.
pub fn forward(link: &str) -> bool {
    forward_to(SocketAddr::from((Ipv4Addr::LOCALHOST, PORT)), link).unwrap_or(false)
}

/// Nimmt weitergereichte Links entgegen und ruft `on_link` für jeden auf (in einem Hintergrund-Thread).
/// Schlägt fehl, wenn der Port belegt ist, etwa weil die App schon läuft.
pub fn listen(on_link: impl Fn(String) + Send + 'static) -> Result<()> {
    listen_on(SocketAddr::from((Ipv4Addr::LOCALHOST, PORT)), on_link).map(|_| ())
}

/// Hält fest, dass der Browser einen Rücksprung geliefert hat (ohne den Link selbst, er enthält den Code). Diese
/// Instanz hat weder Terminal noch Fenster, die Datei ist der einzige Hinweis bei der Fehlersuche.
pub fn log_attempt(forwarded: bool) {
    let path = crate::config::dirs().data_local_dir().join("deeplink.log");
    let line = format!(
        "{} Rücksprung empfangen, {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        if forwarded {
            "an die laufende App übergeben"
        } else {
            "keine laufende App gefunden"
        }
    );
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path))
        .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(&path))
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

fn forward_to(addr: SocketAddr, link: &str) -> Result<bool> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    writeln!(stream, "{}", link.trim())?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    Ok(answer.trim() == "ok")
}

fn listen_on(addr: SocketAddr, on_link: impl Fn(String) + Send + 'static) -> Result<SocketAddr> {
    let listener = TcpListener::bind(addr).with_context(|| format!("Port {addr} ist belegt"))?;
    let local = listener.local_addr()?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            let ok = reader.read_line(&mut line).is_ok()
                && line.len() <= MAX_LINK_LEN
                && (is_link(line.trim()) || line.trim() == ACTIVATE);
            let _ = reader.get_mut().write_all(if ok { b"ok\n" } else { b"nein\n" });
            if ok {
                on_link(line.trim().to_string());
            }
        }
    });
    Ok(local)
}

/// Inhalt der `.desktop`-Datei, mit der sich die App unter Linux für `tutoapp://` anmeldet.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn desktop_entry(exe: &Path) -> String {
    let path = exe.display().to_string();
    // Anführungszeichen nur, wenn nötig: `xdg-open` ohne Desktop-Umgebung (z. B. unter niri) wertet sie nicht aus und
    // findet das Programm sonst nicht. Laut Spezifikation darin `"`, `` ` ``, `$` und `\` maskieren.
    let needs_quotes = path
        .chars()
        .any(|c| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c));
    if !needs_quotes {
        return format!(
            "[Desktop Entry]\nType=Application\nName=Tuto Mic\nExec={path} %u\nComment=Unterricht aufnehmen und zusammenfassen\nCategories=Education;AudioVideo;\n\
             MimeType=x-scheme-handler/{SCHEME};\n"
        );
    }
    let quoted: String = path
        .chars()
        .flat_map(|c| match c {
            '"' | '`' | '$' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!(
        "[Desktop Entry]\nType=Application\nName=Tuto Mic\nExec=\"{quoted}\" %u\nComment=Unterricht aufnehmen und zusammenfassen\nCategories=Education;AudioVideo;\n\
         MimeType=x-scheme-handler/{SCHEME};\n"
    )
}

/// `reg add`-Aufrufe, mit denen sich die App unter Windows für `tutoapp://` anmeldet (nur für den
/// aktuellen Benutzer, ohne Administratorrechte).
#[cfg_attr(not(windows), allow(dead_code))]
fn registry_commands(exe: &Path) -> Vec<Vec<String>> {
    let key = format!(r"HKCU\Software\Classes\{SCHEME}");
    let command = format!("\"{}\" \"%1\"", exe.display());
    [
        vec![key.clone(), "/ve".into(), "/d".into(), "URL:Tuto Mic".into()],
        vec![
            key.clone(),
            "/v".into(),
            "URL Protocol".into(),
            "/d".into(),
            String::new(),
        ],
        vec![format!(r"{key}\shell\open\command"), "/ve".into(), "/d".into(), command],
    ]
    .into_iter()
    .map(|args| {
        let mut full = vec!["add".to_string()];
        full.extend(args);
        full.push("/f".into());
        full
    })
    .collect()
}

#[cfg(target_os = "linux")]
mod platform {
    use anyhow::{Context, Result};
    use std::path::Path;
    use std::process::{Command, Stdio};

    const DESKTOP_FILE: &str = "tuto-mic-link.desktop";

    pub fn register(exe: &Path) -> Result<()> {
        let dir = directories::BaseDirs::new()
            .context("kein Home-Verzeichnis gefunden")?
            .data_dir()
            .join("applications");
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(DESKTOP_FILE);
        let content = super::desktop_entry(exe);
        if std::fs::read_to_string(&file).ok().as_deref() != Some(content.as_str()) {
            std::fs::write(&file, content).with_context(|| format!("{} nicht schreibbar", file.display()))?;
        }
        // Beides ist nur eine Verbesserung: Die Desktop-Datei mit `MimeType` genügt den meisten Systemen, auch dem
        // Desktop-Portal, über das Browser unter Wayland Links öffnen. `mimeapps.list` kann schreibgeschützt sein
        // (z. B. von Home Manager verwaltet); dann fragt das System beim ersten Mal, womit geöffnet werden soll.
        let quiet = |cmd: &mut Command| {
            cmd.stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        if !quiet(Command::new("update-desktop-database").arg(&dir)) {
            log::info!("update-desktop-database nicht verfügbar oder fehlgeschlagen");
        }
        let scheme = format!("x-scheme-handler/{}", super::SCHEME);
        if !quiet(Command::new("xdg-mime").args(["default", DESKTOP_FILE, &scheme])) {
            log::warn!(
                "tutoapp:// nicht als Standard eingetragen (mimeapps.list schreibgeschützt oder xdg-mime fehlt)"
            );
        }
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use anyhow::{Context, Result, bail};
    use std::path::Path;
    use std::process::Command;

    pub fn register(exe: &Path) -> Result<()> {
        for args in super::registry_commands(exe) {
            let status = Command::new("reg")
                .args(&args)
                .status()
                .context("reg.exe nicht gefunden")?;
            if !status.success() {
                bail!("Registry-Eintrag für tutoapp:// fehlgeschlagen");
            }
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use anyhow::{Result, bail};
    use std::path::Path;

    /// macOS braucht ein `.app`-Paket mit `CFBundleURLTypes`; bis dahin wird die Adresse eingefügt.
    pub fn register(_exe: &Path) -> Result<()> {
        bail!("tutoapp:// wird auf diesem System noch nicht automatisch registriert")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn recognizes_links() {
        assert!(is_link("tutoapp://callback?code=1&state=2"));
        assert!(!is_link("https://tuto.example"));
        assert!(!is_link("--dev"));
    }

    #[test]
    fn forwards_link_to_running_instance() {
        let (tx, rx) = mpsc::channel();
        let addr = listen_on(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), move |link| {
            tx.send(link).unwrap();
        })
        .unwrap();
        assert!(forward_to(addr, "tutoapp://callback?code=abc&state=s\n").unwrap());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "tutoapp://callback?code=abc&state=s"
        );
        assert!(forward_to(addr, ACTIVATE).unwrap());
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), ACTIVATE);
        // Alles andere wird abgelehnt.
        assert!(!forward_to(addr, "https://evil.example").unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn forward_without_running_instance_fails() {
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(forward_to(free, "tutoapp://callback").is_err());
    }

    #[test]
    fn desktop_entry_without_special_chars_is_unquoted() {
        let entry = desktop_entry(Path::new("/home/lena/.local/bin/tuto-mic"));
        assert!(entry.contains("Exec=/home/lena/.local/bin/tuto-mic %u\n"), "{entry}");
    }

    #[test]
    fn desktop_entry_quotes_path() {
        let entry = desktop_entry(Path::new("/opt/Tuto Mic/$bin/tuto-mic"));
        // Sichtbar: Das Desktop-Portal blendet versteckte Programme in seiner Auswahl aus.
        assert!(!entry.contains("NoDisplay"));
        assert!(entry.contains("Exec=\"/opt/Tuto Mic/\\$bin/tuto-mic\" %u\n"), "{entry}");
        assert!(entry.contains("MimeType=x-scheme-handler/tutoapp;"));
    }

    #[test]
    fn registry_points_to_exe() {
        let cmds = registry_commands(Path::new(r"C:\Programme\Tuto Mic\tuto-mic.exe"));
        assert_eq!(cmds.len(), 3);
        assert!(cmds.iter().all(|c| c[0] == "add" && c.last().unwrap() == "/f"));
        assert_eq!(cmds[1][3], "URL Protocol");
        assert_eq!(cmds[2][1], r"HKCU\Software\Classes\tutoapp\shell\open\command");
        assert_eq!(cmds[2][4], r#""C:\Programme\Tuto Mic\tuto-mic.exe" "%1""#);
    }
}
