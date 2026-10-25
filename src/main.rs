use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tuto_mic::{app, config, deeplink, models, pipeline, prompts, session, tuto};

/// Tuto Mic – Unterricht aufnehmen, lokal transkribieren, pseudonymisieren und zusammenfassen.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Dev-Modus: Prompt-Editor, Datei-Import, Original-Transkript (auch per `TUTO_MIC_DEV=1`).
    #[arg(long)]
    dev: bool,
    /// Admin-Modus: Einstellungen ändern (auch per `TUTO_MIC_ADMIN=1`).
    #[arg(long)]
    admin: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Audiodatei oder Text-Transkript ohne GUI verarbeiten (zum Testen von Modellen und Prompts).
    Process {
        /// `.txt`/`.md`: fertiges Transkript (UTF-8), ohne Whisper direkt pseudonymisieren und zusammenfassen.
        /// Sonst: Audiodatei (mp3, wav, flac, ogg, m4a, …), die zuerst transkribiert wird.
        file: PathBuf,
        /// Nur transkribieren und pseudonymisieren.
        #[arg(long)]
        no_llm: bool,
    },
    /// Whisper- und VAD-Modell herunterladen.
    Download,
    /// Pfade von Konfiguration, Prompts und Sessions anzeigen.
    Paths,
    /// Verbindung zu Tuto und Upload von Zusammenfassungen.
    Tuto {
        #[command(subcommand)]
        cmd: tuto::cli::TutoCmd,
    },
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,zbus=warn,tracing=warn")).init();
    whisper_rs::install_logging_hooks();
    // Vom Browser gestartet mit dem Rücksprung von Tuto: an die laufende App weitergeben.
    if let Some(link) = std::env::args().nth(1).filter(|a| deeplink::is_link(a)) {
        let forwarded = deeplink::forward(&link);
        deeplink::log_attempt(forwarded);
        if !forwarded {
            // Kein Fehlercode: `xdg-open` würde den Link sonst an den Browser weiterreichen, der ihn wieder an
            // xdg-open gibt.
            eprintln!("Tuto Mic läuft nicht – bitte die App öffnen und noch einmal verbinden.");
        }
        return Ok(());
    }
    let cli = Cli::parse();
    let dev = cli.dev || std::env::var("TUTO_MIC_DEV").is_ok_and(|v| v == "1");
    let admin = cli.admin || std::env::var("TUTO_MIC_ADMIN").is_ok_and(|v| v == "1");

    match cli.cmd {
        None => {
            let options = eframe::NativeOptions {
                viewport: eframe::egui::ViewportBuilder::default()
                    .with_title("Tuto Mic")
                    .with_inner_size([960.0, 820.0])
                    .with_min_inner_size([560.0, 520.0])
                    .with_icon(tuto_mic::theme::window_icon())
                    .with_app_id("tuto-mic"),
                ..Default::default()
            };
            eframe::run_native(
                "Tuto Mic",
                options,
                Box::new(move |cc| Ok(Box::new(app::TutoMicApp::new(&cc.egui_ctx, app::Mode { admin, dev })))),
            )
            .map_err(|e| anyhow::anyhow!("GUI-Fehler: {e}"))
        }
        Some(Cmd::Process { file, no_llm }) => {
            let opts = pipeline::Options {
                cfg: config::Config::load()?,
                summary_prompt: prompts::load(),
                skip_llm: no_llm,
                keep_original_transcript: true,
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
                &pipeline::Source::import(file),
                &opts,
                &tx,
                &notify,
                &Arc::new(AtomicBool::new(false)),
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
            let cfg = config::Config::load()?;
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
        Some(Cmd::Tuto { cmd }) => tuto::cli::run(cmd, &config::Config::load()?),
        Some(Cmd::Paths) => {
            println!("Konfiguration: {}", config::Config::path().display());
            println!("Prompts:       {}", prompts::override_dir().display());
            println!("Sessions:      {}", session::sessions_dir().display());
            println!("Modelle:       {}", config::models_dir().display());
            Ok(())
        }
    }
}
