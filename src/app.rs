use crate::config::{self, Config};
use crate::control::{Command, RecState};
use crate::icons::{self, Icon};
use crate::pipeline::{self, Outcome, Progress, Source};
use crate::prompts;
use crate::recorder::Recorder;
use crate::tuto_panel::TutoPanel;
use crate::{llm, models, session};
use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Recording,
    Result,
    Settings,
    Prompts,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResultView {
    Summary,
    TranscriptAnon,
    Transcript,
    Log,
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
        progress: Arc<std::sync::Mutex<String>>,
    },
    Finished(String),
}

pub struct TutoMicApp {
    cfg: Config,
    dev: bool,
    tab: Tab,
    recorder: Option<(Recorder, PathBuf)>,
    processing: Option<Processing>,
    outcome: Option<Outcome>,
    result_view: ResultView,
    error: Option<String>,
    delete_minutes: u32,
    confirm_delete: bool,
    download: Option<Download>,
    ollama_models: Result<Vec<String>, String>,
    class_list_buf: String,
    prompt_buf: String,
    /// Zuletzt angezeigter Aufnahmefehler, damit „Schließen“ ihn nicht im nächsten Frame wieder öffnet.
    rec_error_seen: Option<String>,
    tuto: TutoPanel,
    /// Hinweis ohne Fehlercharakter, z. B. nach einem Abbruch.
    notice: Option<String>,
}

impl TutoMicApp {
    pub fn new(dev: bool) -> Self {
        let (cfg, error) = match Config::load() {
            Ok(cfg) => (cfg, None),
            Err(e) => (
                Config::default(),
                Some(format!("{e:#} – es gelten die Standardwerte, ohne Klassenliste")),
            ),
        };
        let ollama_models = llm::Ollama::new(&cfg.ollama_url).models().map_err(|e| format!("{e:#}"));
        Self {
            class_list_buf: cfg.class_list.join("\n"),
            cfg,
            dev,
            tab: Tab::Recording,
            recorder: None,
            processing: None,
            outcome: None,
            result_view: ResultView::Summary,
            error,
            delete_minutes: 5,
            confirm_delete: false,
            download: None,
            ollama_models,
            prompt_buf: prompts::load(),
            rec_error_seen: None,
            tuto: TutoPanel::new(),
            notice: None,
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
        self.rec_error_seen = None;
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
        let (tx, rx) = mpsc::channel();
        let abort = Arc::new(AtomicBool::new(false));
        let opts = pipeline::Options {
            cfg: self.cfg.clone(),
            summary_prompt: prompts::load(),
            skip_llm: false,
            keep_original_transcript: self.dev,
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
                        self.result_view = ResultView::Summary;
                        self.tab = Tab::Result;
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
        let progress = Arc::new(std::sync::Mutex::new(String::new()));
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
                anyhow::Ok("Modelle bereit".to_string())
            })();
            let _ = tx.send(res.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.download = Some(Download::Running { rx, progress });
    }

    // ---------- UI ----------

    #[expect(clippy::too_many_lines, reason = "wird bei der Vereinfachung der GUI aufgeteilt")]
    fn ui_recording(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let state = self.state();

        if self.models_missing() {
            ui.group(|ui| {
                ui.label(RichText::new("Spracherkennungs-Modelle fehlen (~550 MB, einmalig).").strong());
                match &self.download {
                    Some(Download::Running { progress, .. }) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(progress.lock().unwrap_or_else(PoisonError::into_inner).clone());
                        });
                    }
                    other => {
                        if let Some(Download::Finished(msg)) = other {
                            ui.label(msg);
                        }
                        if ui.button("Modelle herunterladen").clicked() {
                            self.start_download(&ctx);
                        }
                    }
                }
            });
            ui.add_space(8.0);
        }

        let (color, icon, title) = match state {
            RecState::Idle => (Color32::from_gray(110), None, "Bereit"),
            RecState::Recording => (Color32::from_rgb(200, 40, 40), Some(Icon::Record), "AUFNAHME"),
            RecState::Paused => (Color32::from_rgb(210, 140, 0), Some(Icon::Pause), "PAUSIERT"),
            RecState::Processing => (Color32::from_rgb(40, 110, 200), None, "Verarbeitung …"),
        };
        let elapsed = self.recorder.as_ref().map(|(r, _)| r.recorded()).unwrap_or_default();
        egui::Frame::new()
            .fill(color)
            .inner_margin(16.0)
            .corner_radius(10.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if let Some(icon) = icon {
                        icons::inline(ui, icon, 26.0, Color32::WHITE);
                    }
                    ui.label(RichText::new(title).size(28.0).strong().color(Color32::WHITE));
                    if matches!(state, RecState::Recording | RecState::Paused) {
                        let s = elapsed.as_secs();
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60))
                                    .size(28.0)
                                    .monospace()
                                    .color(Color32::WHITE),
                            );
                        });
                    }
                });
                if let Some((rec, _)) = &self.recorder {
                    let level = (rec.level() * 6.0).clamp(0.0, 1.0);
                    ui.add(egui::ProgressBar::new(level).desired_height(8.0));
                    ui.label(RichText::new(format!("Mikrofon: {}", rec.device_name)).color(Color32::WHITE));
                }
            });
        ui.add_space(12.0);

        ui.horizontal(|ui| match state {
            RecState::Idle => {
                let enabled = !self.models_missing();
                if icons::button(ui, Icon::Record, Some(icons::RECORD_RED), "Aufnahme starten", enabled).clicked() {
                    self.handle(Command::Start, &ctx);
                }
            }
            RecState::Recording | RecState::Paused => {
                let (icon, label, cmd) = if state == RecState::Paused {
                    (Icon::Play, "Fortsetzen", Command::Resume)
                } else {
                    (Icon::Pause, "Pause", Command::Pause)
                };
                if icons::button(ui, icon, None, label, true).clicked() {
                    self.handle(cmd, &ctx);
                }
                if icons::button(ui, Icon::Stop, None, "Stop & auswerten", true).clicked() {
                    self.handle(Command::Stop, &ctx);
                }
            }
            RecState::Processing => {}
        });

        if matches!(state, RecState::Recording | RecState::Paused) {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label("Die letzten");
                egui::ComboBox::from_id_salt("delete_minutes")
                    .selected_text(format!("{} min", self.delete_minutes))
                    .show_ui(ui, |ui| {
                        for m in [1, 2, 5, 10, 15, 30] {
                            ui.selectable_value(&mut self.delete_minutes, m, format!("{m} min"));
                        }
                    });
                if self.confirm_delete {
                    ui.label(RichText::new("Wirklich unwiderruflich löschen?").strong());
                    if ui.button("Ja, löschen").clicked() {
                        self.handle(
                            Command::DeleteLast {
                                minutes: self.delete_minutes,
                            },
                            &ctx,
                        );
                        self.confirm_delete = false;
                    }
                    if ui.button("Abbrechen").clicked() {
                        self.confirm_delete = false;
                    }
                } else if ui.button("Löschen …").clicked() {
                    self.confirm_delete = true;
                }
            });
            ui.label(
                RichText::new(format!(
                    "Gelöscht wird in {}-s-Schritten, im Zweifel etwas mehr.",
                    self.cfg.segment_secs
                ))
                .small()
                .weak(),
            );
        }

        if self.processing.is_none()
            && let Some(notice) = &self.notice
        {
            ui.add_space(12.0);
            ui.label(RichText::new(notice).weak());
        }
        if let Some(p) = &self.processing {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(&p.step);
            });
            if p.step.starts_with("Transkrib") {
                ui.add(egui::ProgressBar::new(p.whisper_pct as f32 / 100.0).show_percentage());
            }
            if p.abort.load(Ordering::Relaxed) {
                ui.label(RichText::new("Wird abgebrochen …").weak());
            } else if ui.button("Abbrechen").clicked() {
                p.abort.store(true, Ordering::Relaxed);
            }
        }

        if self.dev && state == RecState::Idle {
            ui.add_space(16.0);
            ui.separator();
            ui.label(RichText::new("Dev-Modus").strong());
            // Transkripte brauchen kein Whisper-Modell, daher ist der Button auch ohne Modelle aktiv.
            if ui.button("Audio- oder Textdatei verarbeiten …").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Audio oder Transkript", &pipeline::IMPORT_EXTENSIONS)
                    .add_filter("Audio", pipeline::AUDIO_EXTENSIONS)
                    .add_filter("Transkript", pipeline::TRANSCRIPT_EXTENSIONS)
                    .pick_file()
            {
                let source = Source::import(path);
                if source.needs_whisper() && self.models_missing() {
                    self.error = Some("Für Audiodateien fehlen die Modelle, bitte zuerst herunterladen".into());
                } else {
                    self.start_processing(source, &ctx);
                }
            }
        }
    }

    fn ui_result(&mut self, ui: &mut egui::Ui) {
        let Some(outcome) = &mut self.outcome else {
            ui.label("Noch keine Auswertung. Nach einer Aufnahme erscheint das Ergebnis hier.");
            return;
        };
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.result_view, ResultView::Summary, "Zusammenfassung");
            ui.selectable_value(
                &mut self.result_view,
                ResultView::TranscriptAnon,
                "Transkript (pseudonymisiert)",
            );
            if self.dev {
                ui.selectable_value(&mut self.result_view, ResultView::Transcript, "Transkript (Original)");
                ui.selectable_value(&mut self.result_view, ResultView::Log, "Log");
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(outcome.dir.display().to_string()).small().weak());
            if ui.small_button("Ordner öffnen").clicked() {
                open_path(&outcome.dir);
            }
        });
        ui.separator();
        if self.result_view == ResultView::Summary {
            self.tuto.ui(ui, &self.cfg, outcome.summary.as_deref());
            ui.add_space(6.0);
            match &mut outcome.summary {
                // Bearbeitbar, z. B. zum Kürzen vor dem Senden.
                Some(summary) => {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(summary)
                                .desired_width(f32::INFINITY)
                                .desired_rows(30),
                        );
                    });
                }
                None => {
                    ui.label("Keine Zusammenfassung – siehe Log.");
                }
            }
            return;
        }
        let mut text = match self.result_view {
            ResultView::Summary => unreachable!("oben behandelt"),
            ResultView::TranscriptAnon => outcome.transcript_anon.clone(),
            ResultView::Transcript => outcome.transcript.clone(),
            ResultView::Log => outcome.log.join("\n"),
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .interactive(true)
                    .desired_width(f32::INFINITY)
                    .desired_rows(30),
            );
        });
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("settings")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Ollama-URL");
                ui.text_edit_singleline(&mut self.cfg.ollama_url);
                ui.end_row();

                ui.label("Ollama-Modell");
                ui.horizontal(|ui| {
                    match &self.ollama_models {
                        Ok(models) => {
                            egui::ComboBox::from_id_salt("ollama_model")
                                .selected_text(&self.cfg.ollama_model)
                                .show_ui(ui, |ui| {
                                    for m in models {
                                        ui.selectable_value(&mut self.cfg.ollama_model, m.clone(), m);
                                    }
                                });
                        }
                        Err(e) => {
                            ui.text_edit_singleline(&mut self.cfg.ollama_model);
                            ui.label(RichText::new("Ollama nicht erreichbar").color(Color32::from_rgb(220, 160, 0)))
                                .on_hover_text(e);
                        }
                    }
                    if ui
                        .button("Neu laden")
                        .on_hover_text("Modellliste von Ollama neu abfragen")
                        .clicked()
                    {
                        self.ollama_models = llm::Ollama::new(&self.cfg.ollama_url)
                            .models()
                            .map_err(|e| format!("{e:#}"));
                    }
                });
                ui.end_row();

                ui.label("Sprache");
                ui.text_edit_singleline(&mut self.cfg.language);
                ui.end_row();

                ui.label("Whisper-Modell");
                path_edit(ui, &mut self.cfg.whisper_model);
                ui.end_row();

                ui.label("VAD-Modell");
                path_edit(ui, &mut self.cfg.vad_model);
                ui.end_row();

                ui.label("Segmentlänge");
                ui.add(
                    egui::DragValue::new(&mut self.cfg.segment_secs)
                        .range(5..=60)
                        .suffix(" s"),
                );
                ui.end_row();

                ui.label("Datenschutz");
                ui.checkbox(
                    &mut self.cfg.delete_audio_after_transcription,
                    "Audio nach Transkription löschen",
                );
                ui.end_row();
            });

        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Klassenliste (ein Name pro Zeile) – wird vor jeder KI-Verarbeitung durch Pseudonyme ersetzt:",
            )
            .strong(),
        );
        if ui
            .add(
                egui::TextEdit::multiline(&mut self.class_list_buf)
                    .desired_rows(8)
                    .desired_width(f32::INFINITY),
            )
            .changed()
        {
            self.cfg.class_list = config::parse_class_list(&self.class_list_buf);
        }

        ui.add_space(8.0);
        if ui.button("Speichern").clicked()
            && let Err(e) = self.cfg.save()
        {
            self.error = Some(format!("{e:#}"));
        }
        ui.label(RichText::new(Config::path().display().to_string()).small().weak());
    }

    fn ui_prompts(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Prompt für die Zusammenfassung").strong());
        let overridden = prompts::is_overridden();
        ui.label(
            RichText::new(if overridden {
                format!("Eigene Version aus {}", prompts::override_dir().display())
            } else {
                "Eingebaute Version".to_string()
            })
            .small()
            .weak(),
        );
        ui.horizontal(|ui| {
            if ui.button("Speichern").clicked()
                && let Err(e) = prompts::save(&self.prompt_buf)
            {
                self.error = Some(format!("{e:#}"));
            }
            if ui
                .add_enabled(overridden, egui::Button::new("Auf eingebaute Version zurücksetzen"))
                .clicked()
            {
                if let Err(e) = prompts::reset() {
                    self.error = Some(format!("{e:#}"));
                }
                self.prompt_buf = prompts::load();
            }
        });
        ui.label(
            RichText::new(format!(
                "Das Transkript ersetzt {} (fehlt der Platzhalter, wird es angehängt).",
                prompts::TRANSCRIPT_PLACEHOLDER
            ))
            .small()
            .weak(),
        );
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.prompt_buf)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(30),
            );
        });
    }
}

impl eframe::App for TutoMicApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        if let Some((rec, _)) = &self.recorder {
            if let Some(e) = rec.error()
                && self.rec_error_seen.as_ref() != Some(&e)
            {
                self.error = Some(format!("Aufnahmefehler: {e}"));
                self.rec_error_seen = Some(e);
            }
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Tuto Mic");
                if self.dev {
                    ui.label(RichText::new("DEV").color(Color32::from_rgb(230, 120, 0)).strong());
                }
                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Recording, "Aufnahme");
                ui.selectable_value(&mut self.tab, Tab::Result, "Ergebnis");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Einstellungen");
                if self.dev {
                    ui.selectable_value(&mut self.tab, Tab::Prompts, "Prompts");
                }
            });
        });

        if let Some(e) = self.error.clone() {
            egui::Panel::bottom("error").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Fehler: {e}")).color(Color32::from_rgb(220, 60, 60)));
                    if ui.small_button("Schließen").clicked() {
                        self.error = None;
                    }
                });
            });
        }

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Recording => self.ui_recording(ui),
            Tab::Result => self.ui_result(ui),
            Tab::Settings => self.ui_settings(ui),
            Tab::Prompts => self.ui_prompts(ui),
        });
    }

    fn on_exit(&mut self) {
        // Laufende Aufnahme sauber abschließen, damit das letzte Segment gültig ist.
        if let Some((rec, _)) = self.recorder.take() {
            let _ = rec.stop();
        }
    }
}

fn path_edit(ui: &mut egui::Ui, path: &mut PathBuf) {
    let mut s = path.display().to_string();
    let ok = path.is_file();
    ui.horizontal(|ui| {
        if ui
            .add(egui::TextEdit::singleline(&mut s).desired_width(380.0))
            .changed()
        {
            *path = PathBuf::from(&s);
        }
        if ok {
            ui.label(RichText::new("vorhanden").color(Color32::from_rgb(60, 170, 80)));
        } else {
            ui.label(RichText::new("fehlt").color(Color32::from_rgb(220, 60, 60)));
        }
    });
}

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
