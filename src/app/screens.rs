//! Die Seite für Lehrkräfte: je nach Zustand Start, laufende Aufnahme, Auswertung oder Ergebnis. Die einzelnen
//! Karten sind freie Funktionen mit einfachen Eingaben, damit sie sich ohne Mikrofon testen lassen.

use super::{DeleteLast, Download, TutoMicApp, stage_of};
use crate::control::{Command, RecState};
use crate::icons::{self, Icon};
use crate::live;
use crate::theme::{self, BLUE_600, EMERALD_600, ROSE_50, ROSE_600};
use eframe::egui::{self, Color32, RichText, Sense, Stroke, vec2};
use std::sync::PoisonError;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Inhalt mittig mit höchstens `max_width` Breite.
pub fn centered(ui: &mut egui::Ui, max_width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width().min(max_width);
    let margin = ((ui.available_width() - width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(margin);
        ui.vertical(|ui| {
            ui.set_width(width);
            ui.add_space(24.0);
            add(ui);
            ui.add_space(24.0);
        });
    });
}

impl TutoMicApp {
    pub(super) fn main_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if let Some(e) = self.error.clone()
            && message_card(ui, &e, true)
        {
            self.error = None;
        }
        if let Some(s) = self.success.clone()
            && success_card(ui, &s)
        {
            self.success = None;
        }
        if let Some(n) = self.notice.clone()
            && message_card(ui, &n, false)
        {
            self.notice = None;
        }

        let state = self.state();
        if state == RecState::Idle && self.models_missing() {
            self.models_card(ui, &ctx);
        }

        match state {
            RecState::Idle if self.outcome.is_some() => self.result_cards(ui),
            RecState::Idle => {
                let ready = !self.models_missing();
                if idle_card(ui, ready, |ui| self.tuto.ui_plan(ui)) {
                    self.handle(Command::Start, &ctx);
                }
            }
            RecState::Recording | RecState::Paused => {
                let (elapsed, level, device) = self
                    .recorder
                    .as_ref()
                    .map(|(r, _)| (r.recorded(), r.level(), r.device_name.clone()))
                    .unwrap_or_default();
                let view = RecordingView {
                    paused: state == RecState::Paused,
                    elapsed,
                    level,
                    device: &device,
                    segment_secs: self.cfg.segment_secs,
                    live: self.live.as_ref().map(crate::live::LiveTranscriber::status),
                };
                if let Some(cmd) = recording_card(ui, &view, &mut self.delete) {
                    self.handle(cmd, &ctx);
                }
            }
            RecState::Processing => {
                if let Some(p) = &self.processing {
                    let cancelling = p.abort.load(Ordering::Relaxed);
                    if processing_card(ui, stage_of(&p.step), p.whisper_pct, cancelling) {
                        p.abort.store(true, Ordering::Relaxed);
                    }
                }
            }
        }

        if self.mode.admin && state == RecState::Idle {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Admin:").color(theme::weak_text(ui)));
                if ui.button("Audio- oder Textdatei verarbeiten …").clicked() {
                    self.import_file(&ctx);
                }
            });
        }
    }

    fn models_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Einmalige Einrichtung").font(theme::heading(18.0)));
            ui.label(
                RichText::new(
                    "Für die Spracherkennung wird ein Modell geladen (ca. 1,6 GB). Danach läuft alles ohne Internet.",
                )
                .color(theme::weak_text(ui)),
            );
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
                    if ui.add(theme::primary_button("Spracherkennung herunterladen")).clicked() {
                        self.start_download(ctx);
                    }
                }
            }
        });
        ui.add_space(16.0);
    }

    fn result_cards(&mut self, ui: &mut egui::Ui) {
        let Some(outcome) = &mut self.outcome else { return };
        let limit = self.cfg.summary_limit;
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let title = ui
                .horizontal(|ui| {
                    let title = ui.label(RichText::new("Zusammenfassung").font(theme::heading(22.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(summary) = &outcome.summary {
                            let chars = summary.trim().chars().count();
                            let (fg, bg) = if chars > limit {
                                (ROSE_600, ROSE_50)
                            } else {
                                (theme::weak_text(ui), ui.visuals().faint_bg_color)
                            };
                            theme::badge(ui, &format!("{chars} / {limit} Zeichen"), fg, bg);
                        }
                    });
                    title.id
                })
                .inner;
            ui.label(
                RichText::new("Bitte prüfen und bei Bedarf direkt hier anpassen. Es stehen keine Namen darin.")
                    .color(theme::weak_text(ui)),
            );
            ui.add_space(4.0);
            match &mut outcome.summary {
                Some(summary) => {
                    ui.add(
                        egui::TextEdit::multiline(summary)
                            .desired_width(f32::INFINITY)
                            .desired_rows(12)
                            .margin(egui::Margin::same(12)),
                    )
                    .labelled_by(title);
                }
                None => {
                    ui.label(
                        RichText::new("Es konnte keine Zusammenfassung erstellt werden. Läuft Ollama?").color(ROSE_600),
                    );
                }
            }
        });
        ui.add_space(16.0);
        let summary = outcome.summary.clone();
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            self.tuto.ui_send(ui, &self.cfg, summary.as_deref());
        });
        ui.add_space(16.0);
        // Nicht gesendete Auswertungen nicht liegen lassen: Verwerfen löscht die Daten auf diesem Rechner.
        if self.confirm_discard {
            ui.horizontal(|ui| {
                ui.label("Nicht gesendet. Auswertung verwerfen und die Daten auf diesem Rechner löschen?");
                let danger = egui::Button::new(RichText::new("Verwerfen").color(Color32::WHITE)).fill(ROSE_600);
                if ui.add(danger).clicked() {
                    self.discard_outcome();
                }
                if ui.button("Abbrechen").clicked() {
                    self.confirm_discard = false;
                }
            });
        } else if ui.button("Verwerfen und neue Aufnahme").clicked() {
            self.confirm_discard = true;
        }
    }
}

/// Grüne Bestätigung mit „OK“. Liefert `true`, wenn sie geschlossen wurde.
fn success_card(ui: &mut egui::Ui, text: &str) -> bool {
    let fill = if ui.visuals().dark_mode {
        Color32::from_rgb(0x02, 0x2c, 0x22)
    } else {
        Color32::from_rgb(0xec, 0xfd, 0xf5)
    };
    let mut closed = false;
    egui::Frame::new()
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            // Zeile selbst anordnen: Icon, Text und Knopf sitzen so sicher auf derselben Mittellinie, auch wenn der
            // Text umbricht.
            const ICON: f32 = 18.0;
            const GAP: f32 = 12.0;
            let button = vec2(56.0, 34.0);
            let width = ui.available_width();
            let galley = ui.painter().layout(
                text.to_owned(),
                egui::TextStyle::Body.resolve(ui.style()),
                EMERALD_600,
                width - ICON - button.x - 2.0 * GAP,
            );
            let height = galley.size().y.max(button.y);
            let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
            let mid = rect.center().y;

            let c = egui::pos2(rect.left() + ICON / 2.0, mid);
            ui.painter().circle_filled(c, ICON / 2.0, EMERALD_600);
            let tick = vec![c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.0), c + vec2(4.5, -3.5)];
            ui.painter().line(tick, Stroke::new(2.0, Color32::WHITE));

            let text_pos = egui::pos2(rect.left() + ICON + GAP, mid - galley.size().y / 2.0);
            ui.painter().galley(text_pos, galley.clone(), EMERALD_600);
            // Für Screenreader zusätzlich als Text bekannt machen.
            ui.interact(rect, ui.scope_id().with("success"), Sense::hover())
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));

            let button_rect = egui::Rect::from_center_size(egui::pos2(rect.right() - button.x / 2.0, mid), button);
            closed = ui.put(button_rect, egui::Button::new("OK")).clicked();
        });
    ui.add_space(12.0);
    closed
}

/// Fehler- oder Hinweiskarte mit „Schließen“. Liefert `true`, wenn sie geschlossen wurde.
fn message_card(ui: &mut egui::Ui, text: &str, error: bool) -> bool {
    let (fill, fg) = if error {
        (
            if ui.visuals().dark_mode {
                Color32::from_rgb(0x4c, 0x05, 0x19)
            } else {
                ROSE_50
            },
            ROSE_600,
        )
    } else {
        (ui.visuals().faint_bg_color, theme::weak_text(ui))
    };
    let mut closed = false;
    egui::Frame::new()
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                // Platz für den Knopf rechts frei lassen, sonst läuft der umbrochene Text darunter.
                let width = (ui.available_width() - 90.0).max(100.0);
                ui.allocate_ui(egui::vec2(width, 0.0), |ui| {
                    ui.add(egui::Label::new(RichText::new(text).color(fg)).wrap());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    closed = ui.small_button("Schließen").clicked();
                });
            });
        });
    ui.add_space(12.0);
    closed
}

/// Startkarte mit großem Aufnahmeknopf. Liefert `true`, wenn die Aufnahme starten soll.
pub(super) fn idle_card(ui: &mut egui::Ui, ready: bool, extra: impl FnOnce(&mut egui::Ui)) -> bool {
    let mut start = false;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Neue Unterrichtsstunde").font(theme::heading(26.0)));
            ui.label(
                RichText::new(
                    "Tuto Mic nimmt die Stunde auf, schreibt sie auf diesem Rechner mit und fasst sie zusammen. \
                     Aufnahme und Namen verlassen den Rechner nicht.",
                )
                .color(theme::weak_text(ui)),
            );
            ui.add_space(12.0);
            extra(ui);
            ui.add_space(8.0);
            start = record_button(ui, ready).clicked();
            ui.add_space(8.0);
            ui.label(RichText::new("Aufnahme starten").font(theme::heading(16.0)));
            if !ready {
                ui.label(
                    RichText::new("Erst die Spracherkennung herunterladen")
                        .small()
                        .color(theme::weak_text(ui)),
                );
            }
            ui.add_space(8.0);
        });
    });
    start
}

/// Runder roter Aufnahmeknopf.
fn record_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let size = 96.0;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), sense);
    let center = rect.center();
    let base = if enabled { ROSE_600 } else { theme::SLATE_400 };
    let ring = if response.hovered() { 6.0 } else { 0.0 };
    let painter = ui.painter();
    painter.circle_filled(center, size / 2.0, base.gamma_multiply(0.15));
    painter.circle_filled(center, size / 2.0 - 12.0 + ring / 2.0, base);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, "Aufnahme starten"));
    response.on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::Default
    })
}

/// Was die Aufnahmekarte anzeigt.
pub(super) struct RecordingView<'a> {
    pub paused: bool,
    /// Gespeicherte Länge (ohne Pausen, nach Löschungen).
    pub elapsed: Duration,
    pub level: f32,
    pub device: &'a str,
    pub segment_secs: u32,
    /// Fortschritt der Mitschrift, falls sie läuft. Nur die Zeit, nie der Text: Der Bildschirm im Klassenraum
    /// ist oft für alle sichtbar.
    pub live: Option<live::Status>,
}

/// „12:30“ bzw. „1:02:05“.
fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// Zeile unter dem Mikrofon: wie weit die Mitschrift ist.
fn live_line(live: live::Status, elapsed: Duration) -> String {
    if live.failed {
        "Mitschreiben folgt nach der Aufnahme".into()
    } else {
        // Nach „Letzte Minuten verwerfen“ zieht die Mitschrift etwas später nach.
        format!(
            "Mitgeschrieben: {} von {}",
            clock(live.transcribed.min(elapsed)),
            clock(elapsed)
        )
    }
}

/// Karte während der Aufnahme. Liefert den gewählten Befehl.
pub(super) fn recording_card(ui: &mut egui::Ui, view: &RecordingView<'_>, delete: &mut DeleteLast) -> Option<Command> {
    let &RecordingView {
        paused,
        elapsed,
        level,
        device,
        segment_secs,
        live,
    } = view;
    let mut cmd = None;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            if paused {
                theme::badge(ui, "Pausiert", theme::AMBER_600, Color32::from_rgb(0xff, 0xf7, 0xe6));
            } else {
                theme::badge(ui, "Aufnahme läuft", ROSE_600, ROSE_50);
            }
            let s = elapsed.as_secs();
            ui.label(
                RichText::new(format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)).font(theme::heading(56.0)),
            );
            level_meter(ui, if paused { 0.0 } else { level });
            ui.label(
                RichText::new(format!("Mikrofon: {device}"))
                    .small()
                    .color(theme::weak_text(ui)),
            );
            if let Some(live) = live {
                ui.label(
                    RichText::new(live_line(live, elapsed))
                        .small()
                        .color(theme::weak_text(ui)),
                );
            }
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                // Zwei Knöpfe zentriert: Breite schätzen und links auffüllen.
                ui.add_space(((ui.available_width() - 380.0) / 2.0).max(0.0));
                let (icon, label, next) = if paused {
                    (Icon::Play, "Fortsetzen", Command::Resume)
                } else {
                    (Icon::Pause, "Pause", Command::Pause)
                };
                if icons::button(ui, icon, false, label, true).clicked() {
                    cmd = Some(next);
                }
                if icons::button(ui, Icon::Stop, true, "Beenden und auswerten", true).clicked() {
                    cmd = Some(Command::Stop);
                }
            });
        });
        ui.add_space(12.0);
        ui.separator();
        if delete.open {
            ui.horizontal(|ui| {
                let label = ui.label("Die letzten");
                egui::ComboBox::from_id_salt("delete_minutes")
                    .width(80.0)
                    .selected_text(format!("{} min", delete.minutes))
                    .show_ui(ui, |ui| {
                        for m in [1, 2, 5, 10, 15, 30] {
                            ui.selectable_value(&mut delete.minutes, m, format!("{m} min"));
                        }
                    })
                    .response
                    .labelled_by(label.id);
                ui.label("unwiderruflich löschen?");
                let danger = egui::Button::new(RichText::new("Löschen").color(Color32::WHITE)).fill(ROSE_600);
                if ui.add(danger).clicked() {
                    cmd = Some(Command::DeleteLast {
                        minutes: delete.minutes,
                    });
                    delete.open = false;
                }
                if ui.button("Abbrechen").clicked() {
                    delete.open = false;
                }
            });
            ui.label(
                RichText::new(format!(
                    "Gelöscht wird in {segment_secs}-s-Schritten, im Zweifel etwas mehr."
                ))
                .small()
                .color(theme::weak_text(ui)),
            );
        } else if ui
            .add(
                egui::Button::new(RichText::new("Letzte Minuten verwerfen …").color(theme::weak_text(ui))).frame(false),
            )
            .clicked()
        {
            delete.open = true;
        }
    });
    cmd
}

fn level_meter(ui: &mut egui::Ui, level: f32) {
    let width = ui.available_width().min(360.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 8.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, ui.visuals().faint_bg_color);
    let filled = rect.with_max_x(rect.left() + rect.width() * (level * 6.0).clamp(0.0, 1.0));
    painter.rect_filled(filled, 4.0, if level > 0.0 { EMERALD_600 } else { theme::SLATE_400 });
}

/// Karte während der Auswertung mit Schrittliste. Liefert `true` bei „Abbrechen“.
pub(super) fn processing_card(ui: &mut egui::Ui, stage: usize, whisper_pct: i32, cancelling: bool) -> bool {
    const STEPS: [&str; 4] = ["Audio laden", "Mitschreiben", "Namen entfernen", "Zusammenfassen"];
    let mut cancel = false;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Stunde wird ausgewertet").font(theme::heading(22.0)));
        ui.label(
            RichText::new("Das läuft komplett auf diesem Rechner. Bitte das Fenster geöffnet lassen.")
                .color(theme::weak_text(ui)),
        );
        ui.add_space(8.0);
        for (i, label) in STEPS.iter().enumerate() {
            ui.horizontal(|ui| {
                step_marker(ui, i.cmp(&stage));
                let text = if i == 1 && i == stage {
                    format!("{label} ({whisper_pct} %)")
                } else {
                    (*label).to_string()
                };
                let color = if i > stage {
                    theme::weak_text(ui)
                } else {
                    ui.visuals().text_color()
                };
                let text = RichText::new(text).color(color);
                ui.label(if i == stage {
                    text.font(theme::heading(15.0))
                } else {
                    text
                });
            });
        }
        if stage == 1 {
            ui.add(
                egui::ProgressBar::new(whisper_pct as f32 / 100.0)
                    .fill(BLUE_600)
                    .desired_height(6.0),
            );
        }
        ui.add_space(8.0);
        if cancelling {
            ui.label(RichText::new("Wird abgebrochen …").color(theme::weak_text(ui)));
        } else if ui.button("Abbrechen").clicked() {
            cancel = true;
        }
    });
    cancel
}

/// Kreis vor einem Schritt: erledigt (gefüllt mit Haken), aktiv (Ring), offen (grau).
fn step_marker(ui: &mut egui::Ui, state: std::cmp::Ordering) {
    let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
    let c = rect.center();
    let painter = ui.painter();
    match state {
        std::cmp::Ordering::Less => {
            painter.circle_filled(c, 9.0, EMERALD_600);
            let tick = [c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.0), c + vec2(4.5, -3.5)];
            painter.line(tick.to_vec(), Stroke::new(2.0, Color32::WHITE));
        }
        std::cmp::Ordering::Equal => {
            painter.circle_stroke(c, 8.0, Stroke::new(2.5, BLUE_600));
            painter.circle_filled(c, 3.5, BLUE_600);
        }
        std::cmp::Ordering::Greater => {
            painter.circle_stroke(c, 8.0, Stroke::new(1.5, theme::SLATE_400));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_line_shows_only_progress() {
        let status = |secs, failed| live::Status {
            transcribed: Duration::from_secs(secs),
            failed,
        };
        let elapsed = Duration::from_secs(845);
        assert_eq!(
            live_line(status(750, false), elapsed),
            "Mitgeschrieben: 12:30 von 14:05"
        );
        // Kurz nach einer Löschung nie mehr als aufgenommen.
        assert_eq!(
            live_line(status(900, false), elapsed),
            "Mitgeschrieben: 14:05 von 14:05"
        );
        assert_eq!(
            live_line(status(3725, false), Duration::from_secs(3725)),
            "Mitgeschrieben: 1:02:05 von 1:02:05"
        );
        assert_eq!(
            live_line(status(0, true), elapsed),
            "Mitschreiben folgt nach der Aufnahme"
        );
    }
}
