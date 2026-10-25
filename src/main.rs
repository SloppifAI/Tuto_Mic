use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tuto_mic::{app, config, deeplink, gpu, models, pipeline, prompts, session, transcribe, tuto};

/// Tuto Mic – Unterricht aufnehmen, lokal transkribieren, pseudonymisieren und zusammenfassen.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Admin-Modus: Einstellungen, Prompt-Editor, Datei-Import, Original-Transkript (auch per `TUTO_MIC_ADMIN=1`).
    #[arg(long, alias = "dev")]
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
    /// Version, Whisper-Backend (GPU/CPU) und Modelle anzeigen.
    Info,
}

fn main() -> anyhow::Result<()> {
    gpu::init_logging(
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,zbus=warn,tracing=warn"))
            .build(),
    );
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
    let admin = cli.admin
        || ["TUTO_MIC_ADMIN", "TUTO_MIC_DEV"]
            .iter()
            .any(|v| std::env::var(v).is_ok_and(|v| v == "1"));

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
                Box::new(move |cc| Ok(Box::new(app::TutoMicApp::new(&cc.egui_ctx, app::Mode { admin })))),
            )
            .map_err(|e| anyhow::anyhow!("GUI-Fehler: {e}"))
        }
        Some(Cmd::Process { file, no_llm }) => process(file, no_llm),
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
        Some(Cmd::Info) => info(),
    }
}

fn process(file: PathBuf, no_llm: bool) -> anyhow::Result<()> {
    let opts = pipeline::Options {
        cfg: config::Config::load()?,
        summary_prompt: prompts::load(),
        skip_llm: no_llm,
        keep_original_transcript: true,
        // Ist die App verbunden, nutzt auch die CLI die Klassenliste aus Tuto.
        tuto: tuto::store::load().ok().flatten(),
        lesson_context: None,
        // Entwicklerwerkzeug: Ergebnisse im Klartext, damit sie sich ansehen lassen. Die GUI verschlüsselt immer.
        encryption: None,
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

/// Version, GPU und die gültige Konfiguration – für Betreuer, die Lehrkräfte sehen keine Einstellungen.
fn info() -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let state = |p: &std::path::Path| if p.is_file() { "vorhanden" } else { "fehlt" };
    println!("Tuto Mic {}", env!("CARGO_PKG_VERSION"));
    let whisper = &cfg.whisper_model;
    // Erst beim Start von Whisper wählt whisper.cpp das Gerät; ohne Modell nur das eingebaute Backend.
    if whisper.is_file() {
        transcribe::probe_device(whisper)?;
    }
    println!(
        "Whisper-Backend: {}",
        gpu::describe(gpu::compiled_backend(), gpu::detected().as_ref())
    );
    println!("Whisper-Modell:  {} ({})", whisper.display(), state(whisper));
    println!(
        "VAD-Modell:      {} ({})",
        cfg.vad_model.display(),
        state(&cfg.vad_model)
    );
    let file = config::Config::path();
    let enc = tuto_mic::crypt::encrypted_path(&file);
    let stored = if enc.is_file() {
        "verschlüsselt"
    } else if file.is_file() {
        "Klartext"
    } else {
        "nicht vorhanden, Standardwerte"
    };
    println!("\nKonfiguration:   {} ({stored})", enc.display());
    println!("  Tuto:          {}", cfg.tuto_url);
    println!("  Höchstlänge:   {} Zeichen", cfg.summary_limit);
    println!("  Ollama:        {} ({})", cfg.ollama_url, cfg.ollama_model);
    println!(
        "  Audio löschen: {}",
        if cfg.delete_audio_after_transcription {
            "ja"
        } else {
            "nein"
        }
    );
    println!("  Klassenliste:  {} Namen", cfg.class_list.len());
    println!("\nÄnderbar über die Datei oder diese Umgebungsvariablen (haben Vorrang):");
    for (name, what) in config::ENV_VARS {
        let set = if std::env::var_os(name).is_some() {
            "  [gesetzt]"
        } else {
            ""
        };
        println!("  {name:<24} {what}{set}");
    }
    Ok(())
}
