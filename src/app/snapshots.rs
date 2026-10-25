//! Bildschirmfotos aller Zustände ohne Fenster, Mikrofon oder Netzwerk (`egui_kittest` + wgpu).
//!
//! `cargo test ui_preview -- --ignored` legt PNGs unter `target/ui-preview/` ab. Die Tests sind ignoriert, weil sie
//! eine GPU bzw. einen Vulkan-Treiber brauchen, den die CI nicht hat.

use super::screens::{RecordingView, idle_card, processing_card, recording_card};
use super::{DeleteLast, Mode, TutoMicApp};
use crate::live;
use crate::pipeline::Outcome;
use crate::tuto::{Lesson, SchoolClass};
use crate::tuto_panel::TutoPanel;
use eframe::egui;
use egui_kittest::Harness;
use std::time::Duration;

const SUMMARY: &str = "# Mathematik, Klasse 7: Prozentrechnung mit eigenen Daten der Klasse\n\n## Lernziele\n\
- Prozentanteile mit der Dreisatz-Treppe berechnen („Teil durch Ganzes“).\n\
- Ergebnisse vorher schätzen und so unsinnige Ergebnisse erkennen.\n\n## Ablauf der Stunde\n\
- Einstieg: Ergebnisse der Klassenumfrage zum Schulweg an der Tafel.\n\
- Erarbeitung: Anteil der Fahrradfahrer schätzen und berechnen.\n";

fn lessons() -> Vec<Lesson> {
    let class = SchoolClass {
        id: 3,
        name: "7b".into(),
    };
    vec![
        Lesson {
            id: 12,
            subject: "Mathematik".into(),
            topic: "Prozentrechnung".into(),
            held_on: chrono::Local::now().date_naive(),
            class: class.clone(),
            summary: None,
            summary_from_app: false,
            released: false,
        },
        Lesson {
            id: 13,
            subject: "Deutsch".into(),
            topic: "Tschick, Kapitel 12".into(),
            held_on: chrono::Local::now().date_naive(),
            class,
            summary: Some("…".into()),
            summary_from_app: true,
            released: true,
        },
    ]
}

fn app(ctx: &egui::Context, tuto: TutoPanel, outcome: Option<Outcome>, models_present: bool) -> TutoMicApp {
    let mut app = TutoMicApp::for_preview(ctx, Mode::default(), tuto);
    if models_present {
        // Irgendeine vorhandene Datei: Für die Ansicht zählt nur, dass das Modell „da“ ist.
        let file = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        app.cfg.whisper_model.clone_from(&file);
        app.cfg.vad_model = file;
    }
    app.outcome = outcome;
    app
}

fn outcome() -> Outcome {
    Outcome {
        summary: Some(SUMMARY.into()),
        ..Outcome::default()
    }
}

fn save(harness: &mut Harness<'_, Option<TutoMicApp>>, name: &str) {
    harness.run();
    let image = harness.render().expect("Rendern mit wgpu");
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-preview");
    std::fs::create_dir_all(&dir).unwrap();
    image.save(dir.join(format!("{name}.png"))).unwrap();
}

/// Ganze Seite mit Kopfzeile; `setup` baut die App beim ersten Frame (braucht den egui-Kontext).
fn page(
    name: &str,
    dark: bool,
    setup: impl Fn(&egui::Context) -> TutoMicApp + 'static,
    content: impl Fn(&mut TutoMicApp, &mut egui::Ui) + 'static,
) {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(900.0, 1100.0))
        .with_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light })
        .wgpu()
        .build_ui_state(
            move |ui, state: &mut Option<TutoMicApp>| {
                let app = state.get_or_insert_with(|| setup(ui.ctx()));
                content(app, ui);
            },
            None,
        );
    save(&mut harness, name);
}

#[test]
#[ignore = "braucht GPU"]
#[expect(
    clippy::too_many_lines,
    reason = "eine Liste von Ansichten, aufgeteilt wäre sie schwerer zu lesen"
)]
fn ui_preview() {
    page(
        "01-erster-start",
        false,
        |ctx| app(ctx, TutoPanel::offline(), None, false),
        TutoMicApp::draw,
    );
    page(
        "02-bereit",
        false,
        |ctx| app(ctx, TutoPanel::offline(), None, true),
        TutoMicApp::draw,
    );
    page(
        "03-aufnahme",
        false,
        |ctx| app(ctx, TutoPanel::offline(), None, true),
        |a, ui| {
            a.draw_frame(ui, |_, ui| {
                let mut delete = DeleteLast {
                    open: false,
                    minutes: 5,
                };
                let view = RecordingView {
                    paused: false,
                    elapsed: Duration::from_secs(1234),
                    level: 0.08,
                    device: "Standard-Mikrofon",
                    segment_secs: 10,
                    live: Some(live::Status {
                        transcribed: Duration::from_secs(1185),
                        failed: false,
                    }),
                };
                recording_card(ui, &view, &mut delete);
            });
        },
    );
    page(
        "04-aufnahme-loeschen",
        false,
        |ctx| app(ctx, TutoPanel::offline(), None, true),
        |a, ui| {
            a.draw_frame(ui, |_, ui| {
                let mut delete = DeleteLast { open: true, minutes: 5 };
                let view = RecordingView {
                    paused: true,
                    elapsed: Duration::from_secs(1834),
                    level: 0.0,
                    device: "Standard-Mikrofon",
                    segment_secs: 10,
                    live: Some(live::Status {
                        transcribed: Duration::ZERO,
                        failed: true,
                    }),
                };
                recording_card(ui, &view, &mut delete);
            });
        },
    );
    page(
        "05-auswertung",
        false,
        |ctx| app(ctx, TutoPanel::offline(), None, true),
        |a, ui| {
            a.draw_frame(ui, |_, ui| {
                let _ = processing_card(ui, 1, 42, false);
            });
        },
    );
    page(
        "06-ergebnis-nicht-verbunden",
        false,
        |ctx| app(ctx, TutoPanel::offline(), Some(outcome()), true),
        TutoMicApp::draw,
    );
    page(
        "07-ergebnis-verbunden",
        false,
        |ctx| {
            app(
                ctx,
                TutoPanel::demo_connected(lessons(), Some(12)),
                Some(outcome()),
                true,
            )
        },
        TutoMicApp::draw,
    );
    page(
        "08-ergebnis-dunkel",
        true,
        |ctx| {
            app(
                ctx,
                TutoPanel::demo_connected(lessons(), Some(12)),
                Some(outcome()),
                true,
            )
        },
        TutoMicApp::draw,
    );
    page(
        "10-ueberschreiben",
        false,
        |ctx| {
            app(
                ctx,
                TutoPanel::demo_connected(lessons(), Some(13)),
                Some(outcome()),
                true,
            )
        },
        TutoMicApp::draw,
    );
    page(
        "11-neue-stunde",
        false,
        |ctx| app(ctx, TutoPanel::demo_new_lesson(lessons()), Some(outcome()), true),
        TutoMicApp::draw,
    );
    page(
        "12-gesendet",
        false,
        |ctx| {
            let mut a = app(ctx, TutoPanel::demo_connected(lessons(), None), None, true);
            a.success = Some(
                "In Tuto gespeichert: Mathematik, 7b, 25.10.2026. Die Daten auf diesem Rechner wurden gelöscht.".into(),
            );
            a
        },
        TutoMicApp::draw,
    );
    page(
        "13-stundenwahl",
        false,
        |ctx| app(ctx, TutoPanel::demo_timetable(1), None, true),
        TutoMicApp::draw,
    );
    page(
        "15-namen-warnung",
        false,
        |ctx| {
            let outcome = Outcome {
                names_warning: Some(
                    "Die Klassenliste konnte nicht aus Tuto geladen werden – Schülernamen sind womöglich nicht \
                     ersetzt. Bitte die Zusammenfassung vor dem Senden auf Schülernamen prüfen."
                        .into(),
                ),
                ..outcome()
            };
            app(ctx, TutoPanel::offline(), Some(outcome), true)
        },
        TutoMicApp::draw,
    );
    page(
        "14-passwort",
        false,
        |ctx| app(ctx, TutoPanel::offline(), None, true),
        |a, ui| {
            let mut lock = super::lock::LockScreen::default();
            egui::CentralPanel::default().show(ui, |ui| {
                lock.ui(ui, &a.brand);
            });
        },
    );
    page(
        "09-bereit-dunkel",
        true,
        |ctx| app(ctx, TutoPanel::offline(), None, true),
        |a, ui| {
            a.draw_frame(ui, |_, ui| {
                let _ = idle_card(ui, true, |_| {});
            });
        },
    );
}
