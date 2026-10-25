//! Editor für die Zusammenfassung: Markdown mit Hervorhebung beim Schreiben, eine Vorschau ohne Markdown-Zeichen
//! und Knöpfe für Überschrift, Fett und Liste. Die Logik dazu steckt in `crate::markdown`.

use crate::markdown::{self, Kind, Selection};
use crate::theme::{self, BLUE_600};
use eframe::egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use eframe::egui::{self, FontId, RichText};

const BODY: f32 = 15.0;

type Toggle = fn(&str, &Selection) -> (String, Selection);

#[derive(Default)]
pub struct SummaryEditor {
    pub(super) preview: bool,
}

impl SummaryEditor {
    /// `label` benennt das Textfeld für Screenreader.
    pub fn ui(&mut self, ui: &mut egui::Ui, text: &mut String, label: egui::Id) {
        let id = ui.make_persistent_id("summary_editor");
        let mut toggle: Option<Toggle> = None;
        ui.horizontal(|ui| {
            if ui.selectable_label(!self.preview, "Bearbeiten").clicked() {
                self.preview = false;
            }
            if ui.selectable_label(self.preview, "Vorschau").clicked() {
                self.preview = true;
            }
            if !self.preview {
                ui.separator();
                if ui.button("Überschrift").clicked() {
                    toggle = Some(markdown::toggle_heading);
                }
                if ui.button(RichText::new("Fett").font(theme::heading(BODY))).clicked() {
                    toggle = Some(markdown::toggle_bold);
                }
                if ui.button("Liste").clicked() {
                    toggle = Some(markdown::toggle_list);
                }
            }
        });
        ui.add_space(4.0);

        if self.preview {
            egui::Frame::new()
                .stroke(ui.visuals().widgets.inactive.bg_stroke)
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(layout(ui, text, true)).wrap());
                });
            return;
        }

        if let Some(toggle) = toggle {
            apply(ui.ctx(), id, text, toggle);
        }
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
            let mut job = layout(ui, buf.as_str(), false);
            job.wrap.max_width = wrap_width;
            ui.fonts_mut(|f| f.layout_job(job))
        };
        ui.add(
            egui::TextEdit::multiline(text)
                .id(id)
                .desired_width(f32::INFINITY)
                .desired_rows(12)
                .margin(egui::Margin::same(12))
                .layouter(&mut layouter),
        )
        .labelled_by(label);
    }
}

/// Wendet einen Knopf auf die aktuelle Auswahl des Textfelds an und setzt Auswahl und Fokus zurück ins Feld.
fn apply(ctx: &egui::Context, id: egui::Id, text: &mut String, toggle: Toggle) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let end = text.chars().count();
    let sel = state.cursor.char_range().map_or(end..end, |r| {
        let range = r.as_sorted_char_range();
        range.start.0..range.end.0
    });
    let (new, sel) = toggle(text, &sel);
    *text = new;
    state
        .cursor
        .set_char_range(Some(CCursorRange::two(CCursor::new(sel.start), CCursor::new(sel.end))));
    egui::TextEdit::store_state(ctx, id, state);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// Formatierter Text: beim Bearbeiten mit gedämpften Markdown-Zeichen, in der Vorschau ohne sie.
fn layout(ui: &egui::Ui, text: &str, preview: bool) -> LayoutJob {
    let color = ui.visuals().text_color();
    let format = |font: FontId, color| TextFormat {
        font_id: font,
        color,
        ..TextFormat::default()
    };
    let body = format(FontId::proportional(BODY), color);
    let mut job = LayoutJob::default();
    let mut pos = 0;
    for span in markdown::spans(text) {
        if span.range.start > pos {
            job.append(&text[pos..span.range.start], 0.0, body.clone());
        }
        let part = &text[span.range.clone()];
        match span.kind {
            Kind::Marker if preview => {}
            Kind::Marker => job.append(part, 0.0, format(FontId::proportional(BODY), theme::weak_text(ui))),
            Kind::Heading(level) => {
                let size = match level {
                    1 => 22.0,
                    2 => 18.0,
                    _ => 16.0,
                };
                job.append(part, 0.0, format(theme::heading(size), color));
            }
            Kind::Bold => job.append(part, 0.0, format(theme::heading(BODY), color)),
            // Die Schrift hat kein „•“, der Gedankenstrich ist darin.
            Kind::Bullet if preview => job.append("–  ", 0.0, format(theme::heading(BODY), BLUE_600)),
            Kind::Bullet => job.append(part, 0.0, format(theme::heading(BODY), BLUE_600)),
        }
        pos = span.range.end;
    }
    if pos < text.len() {
        job.append(&text[pos..], 0.0, body);
    }
    job
}
