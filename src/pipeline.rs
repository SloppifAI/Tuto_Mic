//! Verarbeitung nach der Aufnahme: Audio → Transkript → Pseudonymisierung → Zusammenfassung
//! → Export-Paket. Läuft in einem Hintergrund-Thread.

use crate::anonymize::Pseudonymizer;
use crate::config::Config;
use crate::llm::Ollama;
use crate::{audio, session, transcribe};
use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::time::Instant;

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
        let msg = match run(source, &opts, &tx, &notify, abort) {
            Ok(outcome) => Progress::Done(Box::new(outcome)),
            Err(e) => Progress::Failed(format!("{e:#}")),
        };
        let _ = tx.send(msg);
        notify();
    });
}

pub fn run(
    source: Source,
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
    let (dir, samples) = match &source {
        Source::Session(dir) => (dir.clone(), audio::load_segments(&session::audio_dir(dir))?),
        Source::File(path) => {
            let label = path.file_stem().and_then(|s| s.to_str());
            let dir = session::create(label)?;
            (dir, audio::load_file(path)?)
        }
    };
    let audio_secs = samples.len() as f64 / audio::WHISPER_RATE as f64;
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
    let transcript = transcribe::to_text(&segments);
    std::fs::write(dir.join("transcript.txt"), &transcript)?;
    log.push(format!(
        "Transkription: {} Segmente in {:.1} s",
        segments.len(),
        t.elapsed().as_secs_f64()
    ));

    if matches!(source, Source::Session(_)) && cfg.delete_audio_after_transcription {
        std::fs::remove_dir_all(session::audio_dir(&dir))?;
        log.push("Audio nach Transkription gelöscht".into());
    }

    step("Pseudonymisieren …".into());
    let transcript_anon = Pseudonymizer::new(&cfg.class_list).apply(&transcript);
    std::fs::write(dir.join("transcript.anon.txt"), &transcript_anon)?;

    let mut outcome = Outcome {
        dir: dir.clone(),
        transcript,
        transcript_anon,
        ..Default::default()
    };

    if !opts.skip_llm {
        let ollama = Ollama::new(&cfg.ollama_url);
        step(format!("Zusammenfassung mit {} …", cfg.ollama_model));
        match ollama.generate(
            &cfg.ollama_model,
            &format!("{}{}", opts.summary_prompt, outcome.transcript_anon),
            cfg.num_ctx,
        ) {
            Ok(g) => {
                log.push(format!(
                    "Zusammenfassung ({}): {} Prompt-/{} Output-Tokens, {:.0} s",
                    cfg.ollama_model, g.prompt_tokens, g.output_tokens, g.secs
                ));
                std::fs::write(dir.join("summary.md"), &g.text)?;
                outcome.summary = Some(g.text);
            }
            Err(e) => log.push(format!("Zusammenfassung fehlgeschlagen: {e:#}")),
        }
    }

    // Export-Paket für die spätere Aufgabenerstellung (z. B. Cloud-Modell): nur pseudonymisierte
    // Daten, kein Audio, kein Original-Transkript, keine Namenszuordnung.
    let export = json!({
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
    });
    std::fs::write(dir.join("export.json"), serde_json::to_string_pretty(&export)?)?;
    std::fs::write(dir.join("log.txt"), log.join("\n") + "\n")?;
    outcome.log = log;
    step("Fertig".into());
    Ok(outcome)
}
