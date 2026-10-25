mod anonymize;
mod app;
mod audio;
mod config;
mod control;
mod icons;
mod llm;
mod models;
mod pipeline;
mod prompts;
mod recorder;
mod session;
mod transcribe;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Tuto Mic – Unterricht aufnehmen, lokal transkribieren, pseudonymisieren und zusammenfassen.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Dev-Modus: Prompt-Editor, Datei-Import, Original-Transkript (auch per TUTO_MIC_DEV=1).
    #[arg(long)]
    dev: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Audiodatei ohne GUI verarbeiten (zum Testen von Modellen und Prompts).
    Process {
        audio: PathBuf,
        /// Nur transkribieren und pseudonymisieren.
        #[arg(long)]
        no_llm: bool,
    },
    /// Whisper- und VAD-Modell herunterladen.
    Download,
    /// Pfade von Konfiguration, Prompts und Sessions anzeigen.
    Paths,
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    whisper_rs::install_logging_hooks();
    let cli = Cli::parse();
    let dev = cli.dev || std::env::var("TUTO_MIC_DEV").is_ok_and(|v| v == "1");

    match cli.cmd {
        None => {
            let options = eframe::NativeOptions {
                viewport: eframe::egui::ViewportBuilder::default()
                    .with_title("Tuto Mic")
                    .with_inner_size([820.0, 620.0]),
                ..Default::default()
            };
            eframe::run_native(
                "Tuto Mic",
                options,
                Box::new(move |_cc| Ok(Box::new(app::TutoMicApp::new(dev)))),
            )
            .map_err(|e| anyhow::anyhow!("GUI-Fehler: {e}"))
        }
        Some(Cmd::Process { audio, no_llm }) => {
            let opts = pipeline::Options {
                cfg: config::Config::load(),
                summary_prompt: prompts::load(),
                skip_llm: no_llm,
            };
            let (tx, rx) = std::sync::mpsc::channel();
            let printer = std::thread::spawn(move || {
                let mut last = -1;
                for msg in rx {
                    if let pipeline::Progress::Whisper(p) = msg
                        && p / 10 != last
                    {
                        last = p / 10;
                        eprintln!("  Whisper {p} %");
                    }
                }
            });
            let notify: pipeline::Notify = Arc::new(|| {});
            let outcome = pipeline::run(
                pipeline::Source::File(audio),
                &opts,
                &tx,
                &notify,
                Arc::new(AtomicBool::new(false)),
            );
            // Nicht auf `printer` warten: whisper-rs gibt den Fortschritts-Callback (mit einer Sender-Kopie)
            // nicht frei, der Kanal schließt also nie. Der Thread endet mit dem Prozess.
            drop((tx, printer));
            let outcome = outcome?;
            for line in &outcome.log {
                eprintln!("{line}");
            }
            println!("{}", outcome.dir.display());
            Ok(())
        }
        Some(Cmd::Download) => {
            let cfg = config::Config::load();
            for (spec, dest) in [(models::WHISPER, &cfg.whisper_model), (models::VAD, &cfg.vad_model)] {
                if dest.is_file() {
                    eprintln!("{} vorhanden: {}", spec.label, dest.display());
                    continue;
                }
                eprintln!("Lade {} …", spec.label);
                models::download(spec.url, dest, |_, _| {})?;
            }
            Ok(())
        }
        Some(Cmd::Paths) => {
            println!("Konfiguration: {}", config::Config::path().display());
            println!("Prompts:       {}", prompts::override_dir().display());
            println!("Sessions:      {}", session::sessions_dir().display());
            println!("Modelle:       {}", config::models_dir().display());
            Ok(())
        }
    }
}
