//! Verarbeitung nach der Aufnahme: Audio → Transkript → Pseudonymisierung → Zusammenfassung
//! → Export-Paket. Läuft in einem Hintergrund-Thread. Ein importiertes Text-Transkript überspringt die ersten
//! beiden Schritte.

use crate::anonymize::Pseudonymizer;
use crate::config::Config;
use crate::llm::Ollama;
use crate::{audio, gpu, prompts, session, transcribe};
use anyhow::{Context, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

/// Die Verarbeitung wurde auf Wunsch abgebrochen – kein Fehler, sondern eine eigene Rückmeldung.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Auswertung abgebrochen")
    }
}

impl std::error::Error for Cancelled {}

fn check_abort(abort: &AtomicBool) -> Result<()> {
    if abort.load(Ordering::Relaxed) {
        return Err(Cancelled.into());
    }
    Ok(())
}

/// Dateiendungen, die als Audio importiert werden (Dateidialog im Admin-Modus).
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "wav", "flac", "ogg", "m4a", "opus"];
/// Dateiendungen, die als fertiges Text-Transkript importiert werden.
pub const TRANSCRIPT_EXTENSIONS: &[&str] = &["txt", "md"];
/// Beides zusammen, für einen gemeinsamen Filter im Dateidialog.
pub const IMPORT_EXTENSIONS: [&str; 8] = ["mp3", "wav", "flac", "ogg", "m4a", "opus", "txt", "md"];

#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    /// Aufnahme-Session mit Segmenten in `audio/`.
    Session(PathBuf),
    /// Importierte Audiodatei (Admin-Modus / CLI).
    File(PathBuf),
    /// Importiertes Text-Transkript (UTF-8), z. B. Demo-Daten oder zum Prompt-Testen: kein Audio, kein Whisper.
    Transcript(PathBuf),
}

impl Source {
    /// Importierte Datei nach Endung: Transkript bei `.txt`/`.md`, sonst Audio (symphonia erkennt viele Formate,
    /// eine abschließende Liste wäre zu streng).
    pub fn import(path: PathBuf) -> Self {
        let is_transcript = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| TRANSCRIPT_EXTENSIONS.iter().any(|t| e.eq_ignore_ascii_case(t)));
        if is_transcript {
            Self::Transcript(path)
        } else {
            Self::File(path)
        }
    }

    pub fn needs_whisper(&self) -> bool {
        !matches!(self, Self::Transcript(_))
    }
}

pub struct Options {
    pub cfg: Config,
    pub summary_prompt: String,
    pub skip_llm: bool,
    /// Original-Transkript mit Klarnamen als `transcript.txt` behalten (nur Admin-Modus/CLI).
    pub keep_original_transcript: bool,
}

pub enum Progress {
    Step(String),
    Whisper(i32),
    Done(Box<Outcome>),
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub dir: PathBuf,
    pub transcript: String,
    pub transcript_anon: String,
    pub summary: Option<String>,
    pub log: Vec<String>,
}

pub type Notify = Arc<dyn Fn() + Send + Sync>;

pub fn spawn(source: Source, opts: Options, tx: Sender<Progress>, notify: Notify, abort: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let msg = match run(&source, &opts, &tx, &notify, &abort) {
            Ok(outcome) => Progress::Done(Box::new(outcome)),
            Err(e) if e.is::<Cancelled>() => Progress::Cancelled,
            Err(e) => Progress::Failed(format!("{e:#}")),
        };
        let _ = tx.send(msg);
        notify();
    });
}

pub fn run(
    source: &Source,
    opts: &Options,
    tx: &Sender<Progress>,
    notify: &Notify,
    abort: &Arc<AtomicBool>,
) -> Result<Outcome> {
    run_in(&session::sessions_dir(), source, opts, tx, notify, abort)
}

/// Wie [`run`], legt Verzeichnisse für Importe aber unter `sessions` an (Tests: temporäres Verzeichnis).
fn run_in(
    sessions: &Path,
    source: &Source,
    opts: &Options,
    tx: &Sender<Progress>,
    notify: &Notify,
    abort: &Arc<AtomicBool>,
) -> Result<Outcome> {
    let step = |s: String| {
        log::info!("{s}");
        let _ = tx.send(Progress::Step(s));
        notify();
    };
    let mut log = Vec::new();

    let (dir, transcript, audio_secs) = match source {
        Source::Transcript(path) => {
            step("Transkript laden …".into());
            let transcript = load_transcript(path)?;
            log.push(format!(
                "Transkript importiert: {} Wörter, {} Zeichen (kein Audio, keine Transkription)",
                transcript.split_whitespace().count(),
                transcript.chars().count()
            ));
            (session::create_in(sessions, import_label(path))?, transcript, None)
        }
        Source::Session(dir) => {
            let samples = load_audio(&step, &mut log, || audio::load_segments(&session::audio_dir(dir)))?;
            check_abort(abort)?;
            // Bei Abbruch bleibt das Audio der Aufnahme erhalten, damit nichts unwiederbringlich verloren geht.
            let (transcript, secs) = transcribe_samples(samples, &opts.cfg, tx, notify, abort, &step, &mut log)?;
            if opts.cfg.delete_audio_after_transcription {
                let audio_dir = session::audio_dir(dir);
                std::fs::remove_dir_all(&audio_dir)
                    .with_context(|| format!("Audio nicht löschbar: {}", audio_dir.display()))?;
                log.push("Audio nach Transkription gelöscht".into());
            }
            (dir.clone(), transcript, Some(secs))
        }
        Source::File(path) => {
            let samples = load_audio(&step, &mut log, || audio::load_file(path))?;
            check_abort(abort)?;
            let (transcript, secs) = transcribe_samples(samples, &opts.cfg, tx, notify, abort, &step, &mut log)?;
            // Erst nach der Transkription anlegen: Ein Fehler oder Abbruch hinterlässt so kein leeres Verzeichnis.
            (
                session::create_in(sessions, import_label(path))?,
                transcript,
                Some(secs),
            )
        }
    };

    match finish(&dir, transcript, audio_secs, opts, abort, &step, log) {
        Ok(mut outcome) => {
            outcome.dir = dir;
            Ok(outcome)
        }
        Err(e) => {
            // Abgebrochene Importe hinterlassen nichts; eine Aufnahme behält ihr Verzeichnis.
            if e.is::<Cancelled>() && !matches!(source, Source::Session(_)) {
                let _ = std::fs::remove_dir_all(&dir);
            }
            Err(e)
        }
    }
}

fn import_label(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|s| s.to_str())
}

/// Liest ein Text-Transkript. Leer ist ein Fehler, sonst bekäme das Modell nur den Prompt.
fn load_transcript(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Transkript nicht lesbar (UTF-8-Text erwartet): {}", path.display()))?;
    // Ein BOM (z. B. von Windows-Editoren) gehört nicht ins Transkript.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    anyhow::ensure!(!text.trim().is_empty(), "Transkript ist leer: {}", path.display());
    Ok(text.to_string())
}

fn load_audio(
    step: &dyn Fn(String),
    log: &mut Vec<String>,
    load: impl FnOnce() -> Result<Vec<f32>>,
) -> Result<Vec<f32>> {
    step("Audio laden …".into());
    let t = Instant::now();
    let samples = load()?;
    log.push(format!(
        "Audio: {:.0} s, geladen in {:.1} s",
        audio_secs(&samples),
        t.elapsed().as_secs_f64()
    ));
    Ok(samples)
}

fn audio_secs(samples: &[f32]) -> f64 {
    samples.len() as f64 / f64::from(audio::WHISPER_RATE)
}

/// Whisper über das geladene Audio. Liefert das Transkript als Text und die Audiolänge in Sekunden.
fn transcribe_samples(
    samples: Vec<f32>,
    cfg: &Config,
    tx: &Sender<Progress>,
    notify: &Notify,
    abort: &Arc<AtomicBool>,
    step: &dyn Fn(String),
    log: &mut Vec<String>,
) -> Result<(String, f64)> {
    let secs = audio_secs(&samples);
    step(format!("Transkribieren ({:.0} min Audio) …", secs / 60.0));
    let t = Instant::now();
    let progress_tx = tx.clone();
    let progress_notify = notify.clone();
    let segments = transcribe::transcribe(
        &samples,
        cfg,
        move |p| {
            let _ = progress_tx.send(Progress::Whisper(p));
            progress_notify();
        },
        abort.clone(),
    )
    // whisper.cpp meldet einen Abbruch als allgemeinen Fehler.
    .map_err(|e| {
        if abort.load(Ordering::Relaxed) {
            Cancelled.into()
        } else {
            e
        }
    })?;
    drop(samples);
    log.push(format!(
        "Whisper: {}",
        gpu::describe(gpu::compiled_backend(), gpu::detected().as_ref())
    ));
    log.push(format!(
        "Transkription: {} Segmente in {:.1} s",
        segments.len(),
        t.elapsed().as_secs_f64()
    ));
    Ok((transcribe::to_text(&segments), secs))
}

/// Alles nach der Transkription: Pseudonymisieren, Zusammenfassen, Dateien schreiben. Braucht kein Whisper
/// und ist daher ohne Modelle testbar. `audio_secs` ist `None` bei importierten Text-Transkripten.
fn finish(
    dir: &Path,
    transcript: String,
    audio_secs: Option<f64>,
    opts: &Options,
    abort: &Arc<AtomicBool>,
    step: &dyn Fn(String),
    mut log: Vec<String>,
) -> Result<Outcome> {
    let cfg = &opts.cfg;
    let write = |name: &str, content: &str| {
        let path = dir.join(name);
        std::fs::write(&path, content).with_context(|| format!("Datei nicht schreibbar: {}", path.display()))
    };

    if opts.keep_original_transcript {
        write("transcript.txt", &transcript)?;
    }

    step("Pseudonymisieren …".into());
    if cfg.class_list.is_empty() {
        log.push("Warnung: Klassenliste ist leer – Schülernamen werden nicht ersetzt".into());
    }
    let pseudonymizer = Pseudonymizer::new(&cfg.class_list);
    let transcript_anon = pseudonymizer.apply(&transcript);
    write("transcript.anon.txt", &transcript_anon)?;

    let mut outcome = Outcome {
        transcript,
        transcript_anon,
        ..Outcome::default()
    };

    if !opts.skip_llm {
        check_abort(abort)?;
        step(format!("Zusammenfassung mit {} …", cfg.ollama_model));
        let prompt = prompts::render(&opts.summary_prompt, &outcome.transcript_anon);
        match generate_abortable(cfg, prompt, abort)? {
            Ok(g) => {
                log.push(format!(
                    "Zusammenfassung ({}): {} Prompt-/{} Output-Tokens, {:.0} s",
                    cfg.ollama_model, g.prompt_tokens, g.output_tokens, g.secs
                ));
                // Auch die Ausgabe des Modells filtern: Namen können über Lücken der Pseudonymisierung oder einen
                // geänderten Prompt hineingeraten.
                let summary = pseudonymizer.apply(&g.text);
                let chars = summary.chars().count();
                if chars > cfg.summary_limit {
                    log.push(format!(
                        "Warnung: Zusammenfassung hat {chars} Zeichen, Tuto nimmt höchstens {} an",
                        cfg.summary_limit
                    ));
                }
                write("summary.md", &summary)?;
                outcome.summary = Some(summary);
            }
            Err(e) => log.push(format!("Zusammenfassung fehlgeschlagen: {e:#}")),
        }
    }

    let export = export_json(opts, &outcome, audio_secs);
    write("export.json", &serde_json::to_string_pretty(&export)?)?;
    write("log.txt", &(log.join("\n") + "\n"))?;
    outcome.log = log;
    step("Fertig".into());
    Ok(outcome)
}

/// Fragt Ollama in einem eigenen Thread, damit ein Abbruch nicht auf die (minutenlange) Antwort warten muss.
/// Nach einem Abbruch läuft die Anfrage im Hintergrund zu Ende; ihr Ergebnis wird verworfen.
fn generate_abortable(cfg: &Config, prompt: String, abort: &AtomicBool) -> Result<Result<crate::llm::Generation>> {
    let (tx, rx) = mpsc::channel();
    let (url, model, num_ctx) = (cfg.ollama_url.clone(), cfg.ollama_model.clone(), cfg.num_ctx);
    std::thread::spawn(move || {
        let _ = tx.send(Ollama::new(&url).generate(&model, &prompt, num_ctx));
    });
    loop {
        check_abort(abort)?;
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(result) => return Ok(result),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("Ollama-Anfrage unerwartet beendet"),
        }
    }
}

/// Export-Paket für die spätere Aufgabenerstellung (z. B. Cloud-Modell): nur pseudonymisierte Daten, kein
/// Audio, kein Original-Transkript, keine Namenszuordnung.
///
/// Format `lesson@1` bleibt bei importierten Text-Transkripten unverändert, weil alle Felder erhalten bleiben:
/// `audio_seconds` und `models.transcription` sind dann `null` (kein Audio, kein Whisper). Bisherige Exporte aus
/// Aufnahmen und Audiodateien sehen genauso aus wie vorher; `models.summary` konnte schon immer `null` sein.
fn export_json(opts: &Options, outcome: &Outcome, audio_secs: Option<f64>) -> serde_json::Value {
    let cfg = &opts.cfg;
    json!({
        "format": "tuto-mic/lesson@1",
        "created": chrono::Local::now().to_rfc3339(),
        "app_version": env!("CARGO_PKG_VERSION"),
        "language": cfg.language,
        "audio_seconds": audio_secs.map(f64::round),
        "transcript": outcome.transcript_anon,
        "summary": outcome.summary,
        "models": {
            "transcription": audio_secs.and(cfg.whisper_model.file_name()).map(|f| f.to_string_lossy()),
            "summary": if opts.skip_llm { None } else { Some(&cfg.ollama_model) },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_http::serve_once;

    const TRANSCRIPT: &str = "[00:01] Lena Wagner, bitte an die Tafel.\n[00:05] Lenas Ergebnis ist 25 %.\n";
    /// Lokal praktisch nie belegt: Ollama ist „nicht erreichbar“.
    const NO_OLLAMA: &str = "http://127.0.0.1:9";

    fn options(ollama_url: &str) -> Options {
        Options {
            cfg: Config {
                class_list: vec!["Lena Wagner".into()],
                ollama_url: ollama_url.into(),
                ..Config::default()
            },
            summary_prompt: "Fasse zusammen:\n{{TRANSKRIPT}}\n".into(),
            skip_llm: false,
            keep_original_transcript: false,
        }
    }

    fn run_finish(dir: &Path, opts: &Options) -> Outcome {
        finish(
            dir,
            TRANSCRIPT.into(),
            Some(61.4),
            opts,
            &Arc::default(),
            &|_| {},
            Vec::new(),
        )
        .unwrap()
    }

    fn read(dir: &Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap()
    }

    #[test]
    fn no_real_name_leaves_the_pipeline() {
        // Das Modell gibt trotz Pseudonymisierung einen Namen aus – er muss trotzdem herausgefiltert werden.
        let (url, server) = serve_once(200, r##"{"response":"# Mathe\nLena rechnete 25 % vor."}"##);
        let dir = tempfile::tempdir().unwrap();
        let outcome = run_finish(dir.path(), &options(&url));

        let request: serde_json::Value = serde_json::from_str(&server.join().unwrap().body).unwrap();
        let prompt = request["prompt"].as_str().unwrap();
        assert!(
            prompt.starts_with("Fasse zusammen:\n[00:01] [Schüler A], bitte"),
            "{prompt}"
        );

        assert!(!dir.path().join("transcript.txt").exists());
        for name in ["transcript.anon.txt", "summary.md", "export.json", "log.txt"] {
            let content = read(dir.path(), name);
            assert!(
                !content.contains("Lena") && !content.contains("Wagner"),
                "{name}: {content}"
            );
        }
        assert_eq!(
            outcome.summary.as_deref(),
            Some("# Mathe\n[Schüler A] rechnete 25 % vor.")
        );
        assert!(read(dir.path(), "export.json").contains("\"audio_seconds\": 61.0"));
    }

    #[test]
    fn keeps_original_transcript_only_on_request() {
        let dir = tempfile::tempdir().unwrap();
        let opts = Options {
            skip_llm: true,
            keep_original_transcript: true,
            ..options(NO_OLLAMA)
        };
        run_finish(dir.path(), &opts);
        assert_eq!(read(dir.path(), "transcript.txt"), TRANSCRIPT);
    }

    #[test]
    fn failed_summary_is_logged_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = run_finish(dir.path(), &options(NO_OLLAMA));
        assert_eq!(outcome.summary, None);
        assert!(!dir.path().join("summary.md").exists());
        assert!(read(dir.path(), "log.txt").contains("Zusammenfassung fehlgeschlagen"));
    }

    #[test]
    fn warns_about_empty_class_list_and_long_summary() {
        let body = format!(r#"{{"response":"{}"}}"#, "x".repeat(5001));
        let (url, _server) = serve_once(200, body);
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(&url);
        opts.cfg.class_list.clear();
        opts.cfg.summary_limit = 5000;
        let log = run_finish(dir.path(), &opts).log.join("\n");
        assert!(log.contains("Klassenliste ist leer"), "{log}");
        assert!(log.contains("höchstens 5000"), "{log}");
    }

    #[test]
    fn cancelled_import_leaves_nothing_behind() {
        let sessions = tempfile::tempdir().unwrap();
        let file = sessions.path().join("stunde.txt");
        std::fs::write(&file, TRANSCRIPT).unwrap();
        let source = Source::import(file.clone());
        let (tx, _rx) = std::sync::mpsc::channel();
        let notify: Notify = Arc::new(|| {});
        let err = run_in(
            sessions.path(),
            &source,
            &options(NO_OLLAMA),
            &tx,
            &notify,
            &Arc::new(AtomicBool::new(true)),
        )
        .unwrap_err();
        assert!(err.is::<Cancelled>(), "{err:#}");
        let left: Vec<_> = std::fs::read_dir(sessions.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(left, [file]);
    }

    #[test]
    fn cancel_does_not_wait_for_ollama() {
        // Ein „Ollama“, das die Verbindung annimmt, aber nie antwortet.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let _conn = listener.accept();
            std::thread::sleep(Duration::from_secs(30));
        });
        let abort = Arc::new(AtomicBool::new(false));
        let setter = abort.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            setter.store(true, Ordering::Relaxed);
        });

        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let err = finish(
            dir.path(),
            TRANSCRIPT.into(),
            None,
            &options(&url),
            &abort,
            &|_| {},
            Vec::new(),
        )
        .unwrap_err();
        assert!(err.is::<Cancelled>(), "{err:#}");
        assert!(start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
    }

    fn run_source(sessions: &Path, source: &Source, opts: &Options) -> Result<Outcome> {
        let (tx, _rx) = std::sync::mpsc::channel();
        let notify: Notify = Arc::new(|| {});
        run_in(sessions, source, opts, &tx, &notify, &Arc::default())
    }

    /// Einträge direkt unter `dir`, sortiert.
    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn transcript_source_runs_without_audio_and_whisper() {
        let input = tempfile::tempdir().unwrap();
        let path = input.path().join("mathe stunde.txt");
        // Format der Demo-Transkripte: Absätze statt Zeitstempel, hier mit BOM.
        let text = "\u{feff}So, Ruhe bitte. Lena Wagner, an die Tafel.\n\nZwölf von 48 sind 25 %.\n";
        std::fs::write(&path, text).unwrap();
        let (url, server) = serve_once(200, r##"{"response":"# Prozente\nLena rechnete vor."}"##);
        let sessions = tempfile::tempdir().unwrap();

        let opts = options(&url);
        let outcome = run_source(sessions.path(), &Source::Transcript(path), &opts).unwrap();

        let request: serde_json::Value = serde_json::from_str(&server.join().unwrap().body).unwrap();
        assert_eq!(
            request["prompt"].as_str().unwrap(),
            "Fasse zusammen:\nSo, Ruhe bitte. [Schüler A], an die Tafel.\n\nZwölf von 48 sind 25 %.\n"
        );
        assert_eq!(
            outcome.summary.as_deref(),
            Some("# Prozente\n[Schüler A] rechnete vor.")
        );
        assert!(outcome.dir.starts_with(sessions.path()));
        assert!(outcome.dir.to_string_lossy().ends_with("_mathe_stunde"));
        assert_eq!(
            entries(&outcome.dir),
            ["export.json", "log.txt", "summary.md", "transcript.anon.txt"]
        );

        let export: serde_json::Value = serde_json::from_str(&read(&outcome.dir, "export.json")).unwrap();
        assert_eq!(export["format"], "tuto-mic/lesson@1");
        assert!(export["audio_seconds"].is_null());
        assert!(export["models"]["transcription"].is_null());
        assert_eq!(export["models"]["summary"], opts.cfg.ollama_model.as_str());
        for name in ["transcript.anon.txt", "summary.md", "export.json", "log.txt"] {
            let content = read(&outcome.dir, name);
            assert!(
                !content.contains("Lena") && !content.contains("Wagner"),
                "{name}: {content}"
            );
        }
        assert!(read(&outcome.dir, "log.txt").contains("Transkript importiert: 14 Wörter"));
    }

    #[test]
    fn audio_export_keeps_format() {
        let dir = tempfile::tempdir().unwrap();
        let opts = Options {
            skip_llm: true,
            ..options(NO_OLLAMA)
        };
        run_finish(dir.path(), &opts);
        let export: serde_json::Value = serde_json::from_str(&read(dir.path(), "export.json")).unwrap();
        assert_eq!(export["audio_seconds"], 61.0);
        assert!(export["models"]["transcription"].is_string());
    }

    #[test]
    fn failed_imports_leave_no_session_dir() {
        let input = tempfile::tempdir().unwrap();
        let empty = input.path().join("leer.txt");
        std::fs::write(&empty, " \n\n").unwrap();
        let sessions = tempfile::tempdir().unwrap();
        let opts = options(NO_OLLAMA);

        let err = run_source(sessions.path(), &Source::Transcript(empty), &opts).unwrap_err();
        assert!(format!("{err:#}").contains("Transkript ist leer"), "{err:#}");
        let missing = input.path().join("fehlt.mp3");
        assert!(run_source(sessions.path(), &Source::File(missing), &opts).is_err());
        assert_eq!(entries(sessions.path()), Vec::<String>::new());
    }

    #[test]
    fn import_routes_by_extension() {
        let import = |p: &str| Source::import(PathBuf::from(p));
        assert_eq!(import("a/stunde.txt"), Source::Transcript("a/stunde.txt".into()));
        assert_eq!(import("stunde.MD"), Source::Transcript("stunde.MD".into()));
        assert_eq!(import("stunde.mp3"), Source::File("stunde.mp3".into()));
        assert_eq!(import("stunde.webm"), Source::File("stunde.webm".into()));
        assert_eq!(import("stunde"), Source::File("stunde".into()));
        assert!(!import("stunde.txt").needs_whisper());
        assert!(import("stunde.wav").needs_whisper());
        assert_eq!(
            IMPORT_EXTENSIONS.to_vec(),
            [AUDIO_EXTENSIONS, TRANSCRIPT_EXTENSIONS].concat()
        );
    }
}
