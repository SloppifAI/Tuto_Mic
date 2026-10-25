//! Desktop-Oberfläche. Für Lehrkräfte gibt es genau einen Ablauf auf einer Seite: aufnehmen → auswerten →
//! Zusammenfassung prüfen → an Tuto senden. Einstellungen gibt es nicht in der GUI (zentral per Datei oder
//! Umgebungsvariablen, siehe `config`); Prompt-Editor und Rohdaten nur im Dev-Modus.

mod dev;
mod screens;

use crate::config::Config;
use crate::control::{Command, RecState};
use crate::pipeline::{self, Outcome, Progress, Source};
use crate::recorder::Recorder;
use crate::theme::{self, Brand};
use crate::tuto_panel::TutoPanel;
use crate::{models, prompts, session};
use eframe::egui::{self, RichText};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// Welche Seite angezeigt wird. Lehrkräfte sehen nur `Main`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Main,
    Prompts,
    Details,
}

struct Processing {
    rx: Receiver<Progress>,
    abort: Arc<AtomicBool>,
    step: String,
    whisper_pct: i32,
}

enum Download {
    Running {
        rx: Receiver<Result<String, String>>,
        progress: Arc<Mutex<String>>,
    },
    Finished(String),
}

/// Zustand von „Letzte Minuten verwerfen“ während der Aufnahme.
struct DeleteLast {
    open: bool,
    minutes: u32,
}

/// Wie die App gestartet wurde.
#[derive(Clone, Copy, Default)]
pub struct Mode {
    /// Zusätzlich Prompt-Editor, Datei-Import, Original-Transkript und Log.
    pub dev: bool,
}

pub struct TutoMicApp {
    cfg: Config,
    mode: Mode,
    page: Page,
    brand: Brand,
    recorder: Option<(Recorder, PathBuf)>,
    processing: Option<Processing>,
    outcome: Option<Outcome>,
    error: Option<String>,
    /// Hinweis ohne Fehlercharakter, z. B. nach einem Abbruch.
    notice: Option<String>,
    delete: DeleteLast,
    download: Option<Download>,
    tuto: TutoPanel,
    dev: dev::DevState,
    /// Zuletzt angezeigter Aufnahmefehler, damit „Schließen“ ihn nicht im nächsten Frame wieder öffnet.
    rec_error_seen: Option<String>,
}

impl TutoMicApp {
    pub fn new(ctx: &egui::Context, mode: Mode) -> Self {
        theme::install(ctx);
        let (cfg, error) = match Config::load() {
            Ok(cfg) => (cfg, None),
            Err(e) => (
                Config::default(),
                Some(format!("{e:#} – es gelten die Standardwerte, ohne Klassenliste")),
            ),
        };
        Self {
            dev: dev::DevState::new(),
            cfg,
            mode,
            page: Page::Main,
            brand: Brand::load(ctx),
            recorder: None,
            processing: None,
            outcome: None,
            error,
            notice: None,
            delete: DeleteLast {
                open: false,
                minutes: 5,
            },
            download: None,
            tuto: TutoPanel::connect(),
            rec_error_seen: None,
        }
    }

    /// Für Bildschirmfotos: Standardwerte, ohne Listener, Schlüsselbund oder Konfigurationsdatei.
    #[cfg(test)]
    fn for_preview(ctx: &egui::Context, mode: Mode, tuto: TutoPanel) -> Self {
        theme::install(ctx);
        let cfg = Config::default();
        Self {
            dev: dev::DevState::new(),
            cfg,
            mode,
            page: Page::Main,
            brand: Brand::load(ctx),
            recorder: None,
            processing: None,
            outcome: None,
            error: None,
            notice: None,
            delete: DeleteLast {
                open: false,
                minutes: 5,
            },
            download: None,
            tuto,
            rec_error_seen: None,
        }
    }

    fn state(&self) -> RecState {
        match (&self.recorder, &self.processing) {
            (Some((r, _)), _) if r.is_paused() => RecState::Paused,
            (Some(_), _) => RecState::Recording,
            (None, Some(_)) => RecState::Processing,
            (None, None) => RecState::Idle,
        }
    }

    /// Zentrale Steuerung – später auch von einem Hardware-Taster aufgerufen.
    fn handle(&mut self, cmd: Command, ctx: &egui::Context) {
        let result = match cmd {
            Command::Start => self.start_recording(),
            Command::Pause => {
                if let Some((r, _)) = &self.recorder {
                    r.pause();
                }
                Ok(())
            }
            Command::Resume => {
                if let Some((r, _)) = &self.recorder {
                    r.resume();
                }
                Ok(())
            }
            Command::Stop => self.stop_recording(ctx),
            Command::DeleteLast { minutes } => {
                if let Some((r, _)) = &self.recorder {
                    r.delete_last(Duration::from_secs(u64::from(minutes) * 60));
                }
                Ok(())
            }
        };
        if let Err(e) = result {
            self.error = Some(format!("{e:#}"));
        }
    }

    fn start_recording(&mut self) -> anyhow::Result<()> {
        let dir = session::create_recording()?;
        let rec = match Recorder::start(&session::audio_dir(&dir), self.cfg.segment_secs) {
            Ok(rec) => rec,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
        };
        self.error = None;
        self.notice = None;
        self.rec_error_seen = None;
        self.outcome = None;
        self.recorder = Some((rec, dir));
        Ok(())
    }

    fn stop_recording(&mut self, ctx: &egui::Context) -> anyhow::Result<()> {
        if let Some((rec, dir)) = self.recorder.take() {
            rec.stop()?;
            self.start_processing(Source::Session(dir), ctx);
        }
        Ok(())
    }

    fn start_processing(&mut self, source: Source, ctx: &egui::Context) {
        self.notice = None;
        self.outcome = None;
        let (tx, rx) = mpsc::channel();
        let abort = Arc::new(AtomicBool::new(false));
        let opts = pipeline::Options {
            cfg: self.cfg.clone(),
            summary_prompt: prompts::load(),
            skip_llm: false,
            keep_original_transcript: self.mode.dev,
        };
        let ctx = ctx.clone();
        pipeline::spawn(source, opts, tx, Arc::new(move || ctx.request_repaint()), abort.clone());
        self.processing = Some(Processing {
            rx,
            abort,
            step: "Starte …".into(),
            whisper_pct: 0,
        });
    }

    fn poll(&mut self) {
        if let Some(p) = &mut self.processing {
            while let Ok(msg) = p.rx.try_recv() {
                match msg {
                    Progress::Step(s) => p.step = s,
                    Progress::Whisper(pct) => p.whisper_pct = pct,
                    Progress::Done(outcome) => {
                        self.outcome = Some(*outcome);
                        self.processing = None;
                        return;
                    }
                    Progress::Failed(e) => {
                        self.error = Some(e);
                        self.processing = None;
                        return;
                    }
                    Progress::Cancelled => {
                        self.notice = Some("Auswertung abgebrochen.".into());
                        self.processing = None;
                        return;
                    }
                }
            }
        }
        if let Some(Download::Running { rx, .. }) = &self.download
            && let Ok(res) = rx.try_recv()
        {
            self.download = Some(Download::Finished(res.unwrap_or_else(|e| format!("Fehler: {e}"))));
        }
    }

    fn models_missing(&self) -> bool {
        !self.cfg.whisper_model.is_file() || !self.cfg.vad_model.is_file()
    }

    fn start_download(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let progress = Arc::new(Mutex::new(String::new()));
        let jobs = [
            (models::WHISPER, self.cfg.whisper_model.clone()),
            (models::VAD, self.cfg.vad_model.clone()),
        ];
        let (p, ctx) = (progress.clone(), ctx.clone());
        std::thread::spawn(move || {
            let res = (|| {
                for (spec, dest) in jobs {
                    if dest.is_file() {
                        continue;
                    }
                    models::download(spec.url, &dest, |done, total| {
                        *p.lock().unwrap_or_else(PoisonError::into_inner) = match total {
                            Some(t) => format!("{}: {} / {} MB", spec.label, done >> 20, t >> 20),
                            None => format!("{}: {} MB", spec.label, done >> 20),
                        };
                        ctx.request_repaint();
                    })?;
                }
                anyhow::Ok("Spracherkennung bereit".to_string())
            })();
            let _ = tx.send(res.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.download = Some(Download::Running { rx, progress });
    }

    fn import_file(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Audio oder Transkript", &pipeline::IMPORT_EXTENSIONS)
            .add_filter("Audio", pipeline::AUDIO_EXTENSIONS)
            .add_filter("Transkript", pipeline::TRANSCRIPT_EXTENSIONS)
            .pick_file()
        else {
            return;
        };
        let source = Source::import(path);
        if source.needs_whisper() && self.models_missing() {
            self.error = Some("Für Audiodateien fehlt die Spracherkennung, bitte zuerst herunterladen".into());
        } else {
            self.start_processing(source, ctx);
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(self.brand.logo_for(ui))).max_height(26.0));
            ui.label(
                RichText::new("Mic")
                    .font(theme::heading(24.0))
                    .color(if ui.visuals().dark_mode {
                        theme::BLUE_400
                    } else {
                        theme::BLUE_600
                    }),
            );
            if self.mode.dev {
                theme::badge(ui, "DEV", theme::AMBER_600, theme::ROSE_50);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.mode.dev {
                    ui.menu_button("Menü", |ui| {
                        let mut go = |ui: &mut egui::Ui, page, label: &str| {
                            if ui.button(label).clicked() {
                                self.page = page;
                                ui.close();
                            }
                        };
                        go(ui, Page::Main, "Aufnahme");
                        go(ui, Page::Prompts, "Prompt");
                        go(ui, Page::Details, "Transkript und Log");
                    });
                }
                self.tuto.ui_account(ui, &self.cfg);
            });
        });
    }
}

impl TutoMicApp {
    /// Ganzes Fenster: Kopfzeile und die aktuelle Seite.
    fn draw(&mut self, ui: &mut egui::Ui) {
        self.draw_frame(ui, |app, ui| match app.page {
            Page::Main => app.main_page(ui),
            Page::Prompts => app.prompts_page(ui),
            Page::Details => app.details_page(ui),
        });
    }

    /// Kopfzeile und mittige, scrollbare Inhaltsspalte.
    fn draw_frame(&mut self, ui: &mut egui::Ui, content: impl FnOnce(&mut Self, &mut egui::Ui)) {
        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().window_fill)
                    .inner_margin(egui::Margin::symmetric(20, 12))
                    .stroke(ui.visuals().window_stroke),
            )
            .show(ui, |ui| self.header(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                screens::centered(ui, 720.0, |ui| content(self, ui));
            });
        });
    }
}

impl eframe::App for TutoMicApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        self.tuto.update(&ctx);
        if let Some((rec, _)) = &self.recorder {
            if let Some(e) = rec.error()
                && self.rec_error_seen.as_ref() != Some(&e)
            {
                self.error = Some(format!("Aufnahmefehler: {e}"));
                self.rec_error_seen = Some(e);
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.draw(ui);
    }

    fn on_exit(&mut self) {
        // Laufende Aufnahme sauber abschließen, damit das letzte Segment gültig ist.
        if let Some((rec, _)) = self.recorder.take() {
            let _ = rec.stop();
        }
    }
}

/// Ordner im Dateimanager öffnen.
fn open_path(path: &std::path::Path) {
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(path).spawn();
}

/// Laufender Schritt der Auswertung, für die Schrittanzeige.
fn stage_of(step: &str) -> usize {
    match step {
        s if s.starts_with("Transkrib") => 1,
        s if s.starts_with("Pseudonym") => 2,
        s if s.starts_with("Zusammenfassung") => 3,
        s if s.starts_with("Fertig") => 4,
        _ => 0,
    }
}

#[cfg(test)]
mod snapshots;
