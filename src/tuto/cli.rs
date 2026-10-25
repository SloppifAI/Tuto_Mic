//! `tuto-mic tuto …`: Verbindung und Upload ohne GUI, zum Testen und für Skripte.

use super::auth::{Callback, PendingLogin};
use super::{Client, Session, store};
use crate::{config::Config, deeplink};
use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate};
use clap::Subcommand;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Subcommand)]
pub enum TutoCmd {
    /// Mit Tuto verbinden (Anmeldung im Browser).
    Login,
    /// Verbindung anzeigen.
    Status,
    /// Stunden eines Tages auflisten.
    Lessons {
        /// Datum (JJJJ-MM-TT), Standard: heute.
        #[arg(long)]
        date: Option<NaiveDate>,
    },
    /// Zusammenfassung an eine Stunde senden.
    Send {
        /// `summary.md` oder ein Session-Verzeichnis.
        summary: PathBuf,
        /// ID der Stunde (siehe `tuto lessons`).
        #[arg(long)]
        lesson: u64,
    },
    /// Klassenliste aus Tuto prüfen: zeigt je Klasse nur die Anzahl der Namen, nicht die Namen.
    Roster,
    /// Verbindung trennen (Token bei Tuto ungültig machen und lokal löschen).
    Logout,
}

pub fn run(cmd: TutoCmd, cfg: &Config) -> Result<()> {
    match cmd {
        TutoCmd::Login => login(cfg),
        TutoCmd::Status => {
            match store::load()? {
                Some(s) => println!(
                    "Verbunden mit {} als {} ({}), gültig bis {}",
                    s.base_url, s.user_name, s.role, s.expires_at
                ),
                None => println!("Nicht verbunden. `tuto-mic tuto login` verbindet."),
            }
            Ok(())
        }
        TutoCmd::Lessons { date } => {
            let day = date.unwrap_or_else(|| Local::now().date_naive());
            let lessons = with_session(|c| c.lessons_on(day))?;
            if lessons.is_empty() {
                println!("Keine Stunden am {day}.");
            }
            for l in lessons {
                let flags = match (l.summary.is_some(), l.released) {
                    (_, true) => " [Hausaufgabe freigegeben]",
                    (true, false) => " [hat Zusammenfassung]",
                    _ => "",
                };
                println!(
                    "{:>6}  {}  {}  {}: {}{flags}",
                    l.id, l.held_on, l.class.name, l.subject, l.topic
                );
            }
            Ok(())
        }
        TutoCmd::Send { summary, lesson } => {
            let path = if summary.is_dir() {
                summary.join("summary.md")
            } else {
                summary
            };
            let text = std::fs::read_to_string(&path).with_context(|| format!("{} nicht lesbar", path.display()))?;
            let l = with_session(|c| c.set_summary(lesson, text.trim()))?;
            println!(
                "Zusammenfassung in Tuto gespeichert – {}, {}, {}",
                l.subject,
                l.class.name,
                l.held_on.format("%d.%m.%Y")
            );
            Ok(())
        }
        TutoCmd::Roster => {
            match with_session(super::Client::roster_by_class)? {
                Some(classes) if classes.is_empty() => println!("Keine Klassen."),
                Some(classes) => {
                    for c in classes {
                        println!("{:>8}  {} Namen", c.class.name, c.students.len());
                    }
                }
                None => println!("Diese Tuto-Instanz bietet die Klassenliste noch nicht an."),
            }
            Ok(())
        }
        TutoCmd::Logout => {
            if let Some(session) = store::load()? {
                Client::new(session).logout()?;
            }
            store::delete()?;
            println!("Verbindung getrennt.");
            Ok(())
        }
    }
}

/// Führt `f` mit dem gespeicherten Zugang aus. Bei abgelaufenem Zugang wird er gelöscht.
fn with_session<T>(f: impl FnOnce(&Client) -> Result<T>) -> Result<T> {
    let session = store::load()?.context("Nicht verbunden. Erst `tuto-mic tuto login` ausführen.")?;
    let result = f(&Client::new(session));
    if let Err(e) = &result
        && e.downcast_ref::<super::ApiError>() == Some(&super::ApiError::Unauthorized)
    {
        store::delete()?;
    }
    result
}

fn login(cfg: &Config) -> Result<()> {
    if let Err(e) = deeplink::register() {
        log::warn!("tutoapp:// nicht registriert: {e:#}");
    }
    let pending = PendingLogin::start(&cfg.tuto_url)?;

    // Der Rücksprung kommt entweder über tutoapp:// (vom Browser gestartete zweite Instanz) oder wird
    // eingefügt – was zuerst da ist.
    let (tx, rx) = mpsc::channel::<String>();
    let link_tx = tx.clone();
    if let Err(e) = deeplink::listen(move |link| {
        let _ = link_tx.send(link);
    }) {
        log::warn!("Rücksprung über tutoapp:// nicht möglich: {e:#}");
    }
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if !line.trim().is_empty() {
                let _ = tx.send(line);
            }
        }
    });

    println!("Im Browser bei Tuto anmelden und „Erlauben“ wählen.");
    if webbrowser::open(&pending.authorize_url).is_err() {
        println!("Browser ließ sich nicht öffnen. Bitte diese Adresse aufrufen:");
    }
    println!("{}\n", pending.authorize_url);
    println!("Öffnet sich Tuto Mic danach nicht von selbst, die Adresse aus der Adresszeile des Browsers");
    println!("(beginnt mit tutoapp://callback) hier einfügen und Enter drücken.");

    loop {
        let link = rx
            .recv_timeout(Duration::from_mins(15))
            .context("Keine Rückmeldung von Tuto erhalten")?;
        if pending.is_expired() {
            bail!("Der Verbindungsversuch ist abgelaufen. Bitte neu starten.");
        }
        match pending.parse_callback(&link) {
            Ok(Callback::Code(code)) => {
                let session = pending.finish(&code, &super::device_name())?;
                store::save(&session)?;
                print_connected(&session);
                return Ok(());
            }
            Ok(Callback::Denied) => bail!("Verbindung in Tuto abgelehnt."),
            Err(e) => println!("{e:#} – bitte die vollständige Adresse einfügen."),
        }
    }
}

fn print_connected(s: &Session) {
    println!(
        "Verbunden als {} ({}). Gültig bis {}.",
        s.user_name, s.role, s.expires_at
    );
}
