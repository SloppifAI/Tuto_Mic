//! Verarbeitung nach der Aufnahme: Audio → Transkript → Pseudonymisierung → Zusammenfassung
//! → Export-Paket. Läuft in einem Hintergrund-Thread.

use crate::anonymize::Pseudonymizer;
use crate::config::Config;
use crate::llm::Ollama;
use crate::{audio, prompts, session, transcribe};
use anyhow::{Context, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::time::Instant;

/// Höchstlänge einer Zusammenfassung, die Tuto annimmt (Zeichen).
pub const TUTO_SUMMARY_LIMIT: usize = 5000;

pub enum Source {
    /// Aufnahme-Session mit Segmenten in `audio/`.
    Session(PathBuf),
    /// Importierte Audiodatei (Dev-Modus / CLI).
    File(PathBuf),
}

pub struct Options {
    pub cfg: Config,
    pub summary_prompt: String,
    pub skip_llm: bool,
    /// Original-Transkript mit Klarnamen als `transcript.txt` behalten (nur Dev-Modus/CLI).
    pub keep_original_transcript: bool,
}

pub enum Progress {
    Step(String),
    Whisper(i32),
    Done(Box<Outcome>),
    Failed(String),
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
        let msg = match run(&source, &opts, &tx, &notify, abort) {
            Ok(outcome) => Progress::Done(Box::new(outcome)),
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
    abort: Arc<AtomicBool>,
) -> Result<Outcome> {
    let step = |s: String| {
        log::info!("{s}");
        let _ = tx.send(Progress::Step(s));
        notify();
    };
    let cfg = &opts.cfg;
    let mut log = Vec::new();

    step("Audio laden …".into());
    let t = Instant::now();
    let (dir, samples) = match source {
        Source::Session(dir) => (dir.clone(), audio::load_segments(&session::audio_dir(dir))?),
        Source::File(path) => {
            let label = path.file_stem().and_then(|s| s.to_str());
            let dir = session::create(label)?;
            (dir, audio::load_file(path)?)
        }
    };
    let audio_secs = samples.len() as f64 / f64::from(audio::WHISPER_RATE);
    log.push(format!(
        "Audio: {audio_secs:.0} s, geladen in {:.1} s",
        t.elapsed().as_secs_f64()
    ));

    step(format!("Transkribieren ({:.0} min Audio) …", audio_secs / 60.0));
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
        abort,
    )?;
    drop(samples);
    log.push(format!(
        "Transkription: {} Segmente in {:.1} s",
        segments.len(),
        t.elapsed().as_secs_f64()
    ));

    if matches!(source, Source::Session(_)) && cfg.delete_audio_after_transcription {
        let audio_dir = session::audio_dir(&dir);
        std::fs::remove_dir_all(&audio_dir)
            .with_context(|| format!("Audio nicht löschbar: {}", audio_dir.display()))?;
        log.push("Audio nach Transkription gelöscht".into());
    }

    let mut outcome = finish(&dir, transcribe::to_text(&segments), audio_secs, opts, &step, log)?;
    outcome.dir = dir;
    Ok(outcome)
}

/// Alles nach der Transkription: Pseudonymisieren, Zusammenfassen, Dateien schreiben. Braucht kein Whisper
/// und ist daher ohne Modelle testbar.
fn finish(
    dir: &Path,
    transcript: String,
    audio_secs: f64,
    opts: &Options,
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
        let ollama = Ollama::new(&cfg.ollama_url);
        step(format!("Zusammenfassung mit {} …", cfg.ollama_model));
        match ollama.generate(
            &cfg.ollama_model,
            &prompts::render(&opts.summary_prompt, &outcome.transcript_anon),
            cfg.num_ctx,
        ) {
            Ok(g) => {
                log.push(format!(
                    "Zusammenfassung ({}): {} Prompt-/{} Output-Tokens, {:.0} s",
                    cfg.ollama_model, g.prompt_tokens, g.output_tokens, g.secs
                ));
                // Auch die Ausgabe des Modells filtern: Namen können über Lücken der Pseudonymisierung oder einen
                // geänderten Prompt hineingeraten.
                let summary = pseudonymizer.apply(&g.text);
                let chars = summary.chars().count();
                if chars > TUTO_SUMMARY_LIMIT {
                    log.push(format!(
                        "Warnung: Zusammenfassung hat {chars} Zeichen, Tuto nimmt höchstens {TUTO_SUMMARY_LIMIT} an"
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

/// Export-Paket für die spätere Aufgabenerstellung (z. B. Cloud-Modell): nur pseudonymisierte Daten, kein
/// Audio, kein Original-Transkript, keine Namenszuordnung.
fn export_json(opts: &Options, outcome: &Outcome, audio_secs: f64) -> serde_json::Value {
    let cfg = &opts.cfg;
    json!({
        "format": "tuto-mic/lesson@1",
        "created": chrono::Local::now().to_rfc3339(),
        "app_version": env!("CARGO_PKG_VERSION"),
        "language": cfg.language,
        "audio_seconds": audio_secs.round(),
        "transcript": outcome.transcript_anon,
        "summary": outcome.summary,
        "models": {
            "transcription": cfg.whisper_model.file_name().map(|f| f.to_string_lossy()),
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
        finish(dir, TRANSCRIPT.into(), 61.4, opts, &|_| {}, Vec::new()).unwrap()
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
        let body = format!(r#"{{"response":"{}"}}"#, "x".repeat(TUTO_SUMMARY_LIMIT + 1));
        let (url, _server) = serve_once(200, body);
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(&url);
        opts.cfg.class_list.clear();
        let log = run_finish(dir.path(), &opts).log.join("\n");
        assert!(log.contains("Klassenliste ist leer"), "{log}");
        assert!(log.contains(&format!("höchstens {TUTO_SUMMARY_LIMIT}")), "{log}");
    }
}
