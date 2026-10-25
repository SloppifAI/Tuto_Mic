//! Seiten für Admins (`--admin`) und Entwicklung (`--dev`): Einstellungen, Prompt-Editor, Transkript und Log.
//! Lehrkräfte sehen sie nicht; für sie gelten die Vorgaben aus `config.json`.

use super::{TutoMicApp, open_path};
use crate::config::{self, Config};
use crate::theme::{self, EMERALD_600, ROSE_600};
use crate::{llm, prompts};
use eframe::egui::{self, RichText};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DetailView {
    TranscriptAnon,
    Transcript,
    Log,
}

pub(super) struct AdminState {
    /// Erst auf Knopfdruck abgefragt, damit der Start nicht auf Ollama wartet.
    ollama_models: Option<Result<Vec<String>, String>>,
    class_list_buf: String,
    prompt_buf: String,
    detail: DetailView,
}

impl AdminState {
    pub(super) fn new(cfg: &Config) -> Self {
        Self {
            ollama_models: None,
            class_list_buf: cfg.class_list.join("\n"),
            prompt_buf: prompts::load(),
            detail: DetailView::TranscriptAnon,
        }
    }
}

fn page_title(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).font(theme::heading(26.0)));
    ui.label(RichText::new(subtitle).color(theme::weak_text(ui)));
    ui.add_space(8.0);
}

impl TutoMicApp {
    #[expect(clippy::too_many_lines, reason = "ein Formular, Zeile für Zeile ein Feld")]
    pub(super) fn settings_page(&mut self, ui: &mut egui::Ui) {
        page_title(
            ui,
            "Einstellungen",
            "Gelten für alle, die Tuto Mic auf diesem Rechner nutzen. Lehrkräfte sehen diese Seite nicht.",
        );
        let a = &mut self.admin;
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Klassenliste").font(theme::heading(18.0)));
            ui.label(
                RichText::new(
                    "Ein Name pro Zeile. Diese Namen werden vor jeder KI-Verarbeitung durch Platzhalter ersetzt.",
                )
                .color(theme::weak_text(ui)),
            );
            if ui
                .add(
                    egui::TextEdit::multiline(&mut a.class_list_buf)
                        .desired_rows(6)
                        .desired_width(f32::INFINITY),
                )
                .changed()
            {
                self.cfg.class_list = config::parse_class_list(&a.class_list_buf);
            }
        });
        ui.add_space(16.0);
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Verarbeitung").font(theme::heading(18.0)));
            egui::Grid::new("settings")
                .num_columns(2)
                .spacing([16.0, 10.0])
                .show(ui, |ui| {
                    ui.label("Tuto-Adresse");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.tuto_url).desired_width(360.0));
                    ui.end_row();

                    ui.label("Ollama-Adresse");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.ollama_url).desired_width(360.0));
                    ui.end_row();

                    ui.label("Sprachmodell");
                    ui.horizontal(|ui| {
                        match &a.ollama_models {
                            Some(Ok(models)) => {
                                egui::ComboBox::from_id_salt("ollama_model")
                                    .selected_text(&self.cfg.ollama_model)
                                    .show_ui(ui, |ui| {
                                        for m in models {
                                            ui.selectable_value(&mut self.cfg.ollama_model, m.clone(), m);
                                        }
                                    });
                            }
                            other => {
                                ui.add(egui::TextEdit::singleline(&mut self.cfg.ollama_model).desired_width(200.0));
                                if let Some(Err(e)) = other {
                                    ui.label(RichText::new("Ollama nicht erreichbar").color(ROSE_600))
                                        .on_hover_text(e);
                                }
                            }
                        }
                        if ui.button("Modelle abfragen").clicked() {
                            a.ollama_models = Some(
                                llm::Ollama::new(&self.cfg.ollama_url)
                                    .models()
                                    .map_err(|e| format!("{e:#}")),
                            );
                        }
                    });
                    ui.end_row();

                    ui.label("Höchstlänge Zusammenfassung");
                    ui.add(
                        egui::DragValue::new(&mut self.cfg.summary_limit)
                            .range(500..=50_000)
                            .suffix(" Zeichen"),
                    );
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
                        "Audio nach der Transkription löschen",
                    );
                    ui.end_row();
                });
        });
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if ui.add(theme::primary_button("Speichern")).clicked() {
                match self.cfg.save() {
                    Ok(()) => self.notice = Some("Einstellungen gespeichert.".into()),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            ui.label(
                RichText::new(Config::path().display().to_string())
                    .small()
                    .color(theme::weak_text(ui)),
            );
        });
    }

    pub(super) fn prompts_page(&mut self, ui: &mut egui::Ui) {
        page_title(ui, "Prompt", "Anweisung an das Sprachmodell für die Zusammenfassung.");
        let a = &mut self.admin;
        let overridden = prompts::is_overridden();
        ui.horizontal(|ui| {
            if ui.add(theme::primary_button("Speichern")).clicked()
                && let Err(e) = prompts::save(&a.prompt_buf)
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
                a.prompt_buf = prompts::load();
            }
        });
        ui.label(
            RichText::new(format!(
                "{} · Das Transkript ersetzt {} (fehlt der Platzhalter, wird es angehängt).",
                if overridden {
                    format!("Eigene Version aus {}", prompts::override_dir().display())
                } else {
                    "Eingebaute Version".to_string()
                },
                prompts::TRANSCRIPT_PLACEHOLDER
            ))
            .small()
            .color(theme::weak_text(ui)),
        );
        ui.add(
            egui::TextEdit::multiline(&mut a.prompt_buf)
                .code_editor()
                .desired_width(f32::INFINITY)
                .desired_rows(32),
        );
    }

    pub(super) fn details_page(&mut self, ui: &mut egui::Ui) {
        page_title(ui, "Transkript und Log", "Rohdaten der letzten Auswertung.");
        let Some(outcome) = &self.outcome else {
            ui.label("Noch keine Auswertung.");
            return;
        };
        let a = &mut self.admin;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut a.detail, DetailView::TranscriptAnon, "Pseudonymisiert");
            ui.selectable_value(&mut a.detail, DetailView::Transcript, "Original");
            ui.selectable_value(&mut a.detail, DetailView::Log, "Log");
            if ui.button("Ordner öffnen").clicked() {
                open_path(&outcome.dir);
            }
        });
        let mut text = match a.detail {
            DetailView::TranscriptAnon => outcome.transcript_anon.clone(),
            DetailView::Transcript => outcome.transcript.clone(),
            DetailView::Log => outcome.log.join("\n"),
        };
        ui.add(
            egui::TextEdit::multiline(&mut text)
                .code_editor()
                .desired_width(f32::INFINITY)
                .desired_rows(32),
        );
    }
}

fn path_edit(ui: &mut egui::Ui, path: &mut PathBuf) {
    let mut s = path.display().to_string();
    let ok = path.is_file();
    ui.horizontal(|ui| {
        if ui
            .add(egui::TextEdit::singleline(&mut s).desired_width(300.0))
            .changed()
        {
            *path = PathBuf::from(&s);
        }
        if ok {
            ui.label(RichText::new("vorhanden").color(EMERALD_600));
        } else {
            ui.label(RichText::new("fehlt").color(ROSE_600));
        }
    });
}
