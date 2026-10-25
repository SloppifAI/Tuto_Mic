//! Seiten nur für die Entwicklung (`--dev`): Prompt-Editor, Transkript und Log. Lehrkräfte sehen sie nicht.
//! Einstellungen gibt es bewusst nicht in der GUI (siehe `config`).

use super::{TutoMicApp, open_path};
use crate::prompts;
use crate::theme;
use eframe::egui::{self, RichText};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DetailView {
    TranscriptAnon,
    Transcript,
    Log,
}

pub(super) struct DevState {
    prompt_buf: String,
    detail: DetailView,
}

impl DevState {
    pub(super) fn new() -> Self {
        Self {
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
    pub(super) fn prompts_page(&mut self, ui: &mut egui::Ui) {
        page_title(ui, "Prompt", "Anweisung an das Sprachmodell für die Zusammenfassung.");
        let a = &mut self.dev;
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
        let a = &mut self.dev;
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
