//! Sperrbildschirm: Passwort festlegen (erster Start) oder eingeben. Erst danach lädt die App ihre Konfiguration.

use crate::theme::{self, Brand, ROSE_600};
use crate::unlock::{self, Status};
use eframe::egui::{self, RichText};

#[derive(Default)]
pub struct LockScreen {
    password: String,
    confirm: String,
    error: Option<String>,
    /// Rückfrage „Passwort vergessen – alles zurücksetzen?“ offen.
    confirm_reset: bool,
}

impl LockScreen {
    /// Zeigt den Sperrbildschirm. Liefert `true`, sobald die App entsperrt ist.
    pub fn ui(&mut self, ui: &mut egui::Ui, brand: &Brand) -> bool {
        let status = unlock::status();
        if status == Status::Unlocked {
            return true;
        }
        let setup = status == Status::NeedsSetup;
        let mut unlocked = false;
        super::screens::centered(ui, 460.0, |ui| {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(brand.logo_for(ui))).max_height(40.0));
            });
            ui.add_space(20.0);
            theme::card(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let (title, text) = if setup {
                    (
                        "Passwort festlegen",
                        "Einstellungen, Klassenliste und Prompt werden mit diesem Passwort verschlüsselt. Es wird \
                         bei jedem Start gebraucht und lässt sich nicht wiederherstellen.",
                    )
                } else {
                    ("Tuto Mic entsperren", "Bitte das Passwort für Tuto Mic eingeben.")
                };
                ui.label(RichText::new(title).font(theme::heading(22.0)));
                ui.label(RichText::new(text).color(theme::weak_text(ui)));
                ui.add_space(8.0);

                let label = ui.label("Passwort");
                let field = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.password)
                            .password(true)
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label.id);
                if !ui.memory(|m| m.has_focus(field.id)) && self.password.is_empty() && !self.confirm_reset {
                    field.request_focus();
                }
                let mut submit = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if setup {
                    let label = ui.label("Passwort wiederholen");
                    let again = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.confirm)
                                .password(true)
                                .desired_width(f32::INFINITY),
                        )
                        .labelled_by(label.id);
                    submit = again.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                }
                ui.add_space(8.0);
                let label = if setup { "Festlegen" } else { "Entsperren" };
                if ui.add(theme::primary_button(label)).clicked() || submit {
                    self.error = None;
                    let result = if setup {
                        if self.password == self.confirm {
                            unlock::setup(&self.password)
                        } else {
                            Err(anyhow::anyhow!("Die beiden Passwörter stimmen nicht überein"))
                        }
                    } else {
                        unlock::unlock(&self.password)
                    };
                    match result {
                        Ok(()) => unlocked = true,
                        Err(e) => self.error = Some(format!("{e:#}")),
                    }
                    self.password.clear();
                    self.confirm.clear();
                }
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).color(ROSE_600));
                }
                if !setup {
                    ui.add_space(4.0);
                    self.ui_reset(ui);
                }
            });
        });
        unlocked
    }

    fn ui_reset(&mut self, ui: &mut egui::Ui) {
        if !self.confirm_reset {
            if ui
                .add(egui::Button::new(RichText::new("Passwort vergessen?").color(theme::weak_text(ui))).frame(false))
                .clicked()
            {
                self.confirm_reset = true;
            }
            return;
        }
        ui.label(
            "Ohne Passwort lassen sich Einstellungen, Klassenliste und eigener Prompt nicht wiederherstellen. \
             Zurücksetzen löscht sie; danach gelten die Standardwerte und ein neues Passwort wird festgelegt.",
        );
        ui.horizontal(|ui| {
            let danger = egui::Button::new(RichText::new("Zurücksetzen").color(egui::Color32::WHITE)).fill(ROSE_600);
            if ui.add(danger).clicked() {
                match unlock::reset() {
                    Ok(()) => *self = Self::default(),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            if ui.button("Abbrechen").clicked() {
                self.confirm_reset = false;
            }
        });
    }
}
