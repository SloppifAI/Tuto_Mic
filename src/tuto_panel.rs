//! GUI-Teil der Tuto-Anbindung: verbinden, Stunde wählen oder anlegen, Zusammenfassung senden.
//!
//! Netzwerkaufrufe laufen in Hintergrund-Threads; die Ergebnisse kommen als [`Event`] zurück, damit die
//! Oberfläche nie blockiert.

use crate::config::Config;
use crate::deeplink;
use crate::tuto::auth::{Callback, PendingLogin};
use crate::tuto::{self, ApiError, Client, Lesson, NewLesson, SUMMARY_LIMIT, SchoolClass, Session, store};
use chrono::Local;
use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

const OK_GREEN: Color32 = Color32::from_rgb(60, 170, 90);
const ERROR_RED: Color32 = Color32::from_rgb(220, 60, 60);

enum Event {
    Connected(Session),
    Loaded {
        lessons: Vec<Lesson>,
        classes: Vec<SchoolClass>,
    },
    Created(Lesson),
    Sent(Lesson),
    Failed(String),
    /// Token abgelaufen oder entzogen: Zugang löschen, neu verbinden.
    Disconnected,
}

#[derive(Default)]
struct NewLessonForm {
    open: bool,
    class_id: Option<u64>,
    subject: String,
    topic: String,
}

pub struct TutoPanel {
    session: Option<Session>,
    login: Option<PendingLogin>,
    paste: String,
    links: Receiver<String>,
    events: (Sender<Event>, Receiver<Event>),
    busy: Option<String>,
    lessons: Vec<Lesson>,
    classes: Vec<SchoolClass>,
    loaded: bool,
    selected: Option<u64>,
    form: NewLessonForm,
    confirm_overwrite: bool,
    message: Option<(bool, String)>,
    /// Warum Rücksprünge über tutoapp:// nicht ankommen können (z. B. Port durch eine zweite App belegt).
    listen_error: Option<String>,
}

impl TutoPanel {
    #[expect(
        clippy::new_without_default,
        reason = "startet einen Listener und liest den Schlüsselbund"
    )]
    pub fn new() -> Self {
        // Rücksprünge, die der Browser über tutoapp:// an eine zweite Instanz gibt, landen hier.
        let (link_tx, links) = mpsc::channel();
        let listen_error = deeplink::listen(move |link| {
            log::info!("Rücksprung von Tuto empfangen");
            let _ = link_tx.send(link);
        })
        .err()
        .map(|e| {
            log::warn!("tutoapp://-Rücksprung nicht verfügbar: {e:#}");
            format!("{e:#}")
        });
        let session = store::load().unwrap_or_else(|e| {
            log::warn!("{e:#}");
            None
        });
        Self {
            session,
            login: None,
            paste: String::new(),
            links,
            events: mpsc::channel(),
            busy: None,
            lessons: Vec::new(),
            classes: Vec::new(),
            loaded: false,
            selected: None,
            form: NewLessonForm::default(),
            confirm_overwrite: false,
            message: None,
            listen_error,
        }
    }

    /// Führt `job` im Hintergrund aus und meldet das Ergebnis als Event.
    fn spawn(&mut self, label: &str, ctx: &egui::Context, job: impl FnOnce() -> Event + Send + 'static) {
        self.busy = Some(label.into());
        self.message = None;
        let (tx, ctx) = (self.events.0.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
    }

    fn client_job<T>(
        &mut self,
        label: &str,
        ctx: &egui::Context,
        call: impl FnOnce(&Client) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(T) -> Event + Send + 'static,
    ) {
        let Some(session) = self.session.clone() else { return };
        self.spawn(label, ctx, move || match call(&Client::new(session)) {
            Ok(v) => done(v),
            Err(e) if e.downcast_ref::<ApiError>() == Some(&ApiError::Unauthorized) => Event::Disconnected,
            Err(e) => Event::Failed(format!("{e:#}")),
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(link) = self.links.try_recv() {
            self.handle_link(&link, ctx);
        }
        while let Ok(event) = self.events.1.try_recv() {
            self.busy = None;
            match event {
                Event::Connected(session) => {
                    if let Err(e) = store::save(&session) {
                        self.message = Some((false, format!("{e:#}")));
                    }
                    self.session = Some(session);
                    self.loaded = false;
                }
                Event::Loaded { lessons, classes } => {
                    self.selected = self.selected.filter(|id| lessons.iter().any(|l| l.id == *id));
                    if self.selected.is_none() && lessons.len() == 1 {
                        self.selected = Some(lessons[0].id);
                    }
                    self.lessons = lessons;
                    self.classes = classes;
                    self.loaded = true;
                }
                Event::Created(lesson) => {
                    self.selected = Some(lesson.id);
                    self.lessons.insert(0, lesson);
                    self.form = NewLessonForm::default();
                }
                Event::Sent(lesson) => {
                    self.message = Some((
                        true,
                        format!(
                            "Zusammenfassung in Tuto gespeichert – {}, {}, {}",
                            lesson.subject,
                            lesson.class.name,
                            lesson.held_on.format("%d.%m.%Y")
                        ),
                    ));
                    if let Some(l) = self.lessons.iter_mut().find(|l| l.id == lesson.id) {
                        *l = lesson;
                    }
                    self.confirm_overwrite = false;
                }
                Event::Failed(e) => self.message = Some((false, e)),
                Event::Disconnected => {
                    let _ = store::delete();
                    self.session = None;
                    self.message = Some((false, ApiError::Unauthorized.to_string()));
                }
            }
        }
    }

    fn start_login(&mut self, cfg: &Config) {
        if let Err(e) = deeplink::register() {
            log::warn!("tutoapp:// nicht registriert: {e:#}");
        }
        match PendingLogin::start(&cfg.tuto_url) {
            Ok(login) => {
                if webbrowser::open(&login.authorize_url).is_err() {
                    self.message = Some((
                        false,
                        format!("Browser ließ sich nicht öffnen: {}", login.authorize_url),
                    ));
                }
                self.login = Some(login);
                self.paste.clear();
            }
            Err(e) => self.message = Some((false, format!("{e:#}"))),
        }
    }

    fn handle_link(&mut self, link: &str, ctx: &egui::Context) {
        let Some(login) = self.login.take() else {
            self.message = Some((
                false,
                "Kein Verbindungsversuch aktiv. Bitte „Mit Tuto verbinden“ wählen.".into(),
            ));
            return;
        };
        match login.parse_callback(link) {
            Ok(Callback::Code(code)) => {
                self.spawn("Verbinde mit Tuto …", ctx, move || {
                    match login.finish(&code, &tuto::device_name()) {
                        Ok(session) => Event::Connected(session),
                        Err(e) => Event::Failed(format!("{e:#}")),
                    }
                });
            }
            Ok(Callback::Denied) => self.message = Some((false, "Verbindung in Tuto abgelehnt.".into())),
            Err(e) => {
                self.message = Some((false, format!("{e:#}")));
                self.login = Some(login);
            }
        }
    }

    fn load(&mut self, ctx: &egui::Context) {
        let today = Local::now().date_naive();
        self.client_job(
            "Lade Stunden …",
            ctx,
            move |c| Ok((c.lessons_on(today)?, c.classes()?)),
            |(lessons, classes)| Event::Loaded { lessons, classes },
        );
    }

    /// Zeigt den Bereich „An Tuto senden“ für die aktuelle Zusammenfassung.
    pub fn ui(&mut self, ui: &mut egui::Ui, cfg: &Config, summary: Option<&str>) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        if self.login.is_some() || self.busy.is_some() {
            ctx.request_repaint_after(Duration::from_millis(300));
        }

        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("An Tuto senden").strong());

            if let Some(label) = &self.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(label);
                });
            } else if self.session.is_none() {
                self.ui_login(ui, cfg, &ctx);
            } else {
                self.ui_connected(ui, summary, &ctx);
            }

            if let Some((ok, msg)) = &self.message {
                ui.label(RichText::new(msg).color(if *ok { OK_GREEN } else { ERROR_RED }));
            }
        });
    }

    fn ui_login(&mut self, ui: &mut egui::Ui, cfg: &Config, ctx: &egui::Context) {
        if self.login.as_ref().is_some_and(PendingLogin::is_expired) {
            self.login = None;
        }
        if self.login.is_none() {
            if ui.button("Mit Tuto verbinden").clicked() {
                self.start_login(cfg);
            }
            return;
        }
        ui.label("Bitte im Browser bei Tuto anmelden, „Erlauben“ wählen und die Rückfrage des Browsers bestätigen.");
        if let Some(e) = &self.listen_error {
            ui.label(
                RichText::new(format!(
                    "Der automatische Rücksprung ist blockiert ({e}). Läuft Tuto Mic ein zweites Mal? Dann diese \
                     Instanz schließen und die andere verwenden."
                ))
                .color(ERROR_RED),
            );
        }
        ui.label(
            RichText::new(
                "Falls der Browser die Adresse tutoapp://callback?… anzeigt (z. B. in der Adresszeile oder einer \
                 Fehlermeldung), kann sie auch hier eingefügt werden.",
            )
            .small()
            .weak(),
        );
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.paste)
                    .hint_text("tutoapp://callback?code=…")
                    .desired_width(360.0),
            );
            if ui
                .add_enabled(!self.paste.trim().is_empty(), egui::Button::new("Übernehmen"))
                .clicked()
            {
                let link = std::mem::take(&mut self.paste);
                self.handle_link(&link, ctx);
            }
            if ui.button("Abbrechen").clicked() {
                self.login = None;
            }
        });
    }

    fn ui_connected(&mut self, ui: &mut egui::Ui, summary: Option<&str>, ctx: &egui::Context) {
        let Some(session) = &self.session else { return };
        let user = format!("Verbunden als {}", session.user_name);
        ui.horizontal(|ui| {
            ui.label(RichText::new(user).small().weak());
            if ui.small_button("Neu laden").clicked() {
                self.loaded = false;
            }
            if ui.small_button("Trennen").clicked() {
                let session = self.session.take();
                self.lessons.clear();
                self.loaded = false;
                if let Some(session) = session {
                    // Abmelden bei Tuto im Hintergrund; der lokale Zugang ist sofort weg.
                    std::thread::spawn(move || {
                        let _ = Client::new(session).logout();
                    });
                }
                if let Err(e) = store::delete() {
                    self.message = Some((false, format!("{e:#}")));
                }
            }
        });
        if self.session.is_none() {
            return;
        }
        if !self.loaded {
            self.load(ctx);
            return;
        }

        ui.label(format!("Stunden heute ({}):", Local::now().format("%d.%m.%Y")));
        if self.lessons.is_empty() {
            ui.label(RichText::new("Keine Stunde für heute in Tuto.").weak());
        }
        for lesson in &self.lessons {
            let mut text = format!("{} · {}: {}", lesson.class.name, lesson.subject, lesson.topic);
            if lesson.released {
                text.push_str("  (Hausaufgabe freigegeben)");
            } else if lesson.summary.is_some() {
                text.push_str("  (hat schon eine Zusammenfassung)");
            }
            if ui.radio(self.selected == Some(lesson.id), text).clicked() {
                self.selected = Some(lesson.id);
                self.confirm_overwrite = false;
            }
        }
        self.ui_new_lesson(ui, ctx);

        ui.add_space(6.0);
        let Some(summary) = summary.map(str::trim).filter(|s| !s.is_empty()) else {
            ui.label(RichText::new("Keine Zusammenfassung vorhanden.").weak());
            return;
        };
        let chars = summary.chars().count();
        let too_long = chars > SUMMARY_LIMIT;
        ui.label(
            RichText::new(format!(
                "{chars} / {SUMMARY_LIMIT} Zeichen{}",
                if too_long { " – bitte kürzen" } else { "" }
            ))
            .small()
            .color(if too_long {
                ERROR_RED
            } else {
                ui.visuals().weak_text_color()
            }),
        );
        let lesson = self
            .selected
            .and_then(|id| self.lessons.iter().find(|l| l.id == id))
            .cloned();
        let can_send = lesson.is_some() && !too_long;
        if self.confirm_overwrite
            && let Some(lesson) = &lesson
        {
            ui.label(if lesson.released {
                "Die Hausaufgabe ist schon freigegeben – die Schüler:innen sehen die neue Fassung sofort. Überschreiben?"
            } else {
                "Diese Stunde hat schon eine Zusammenfassung. Überschreiben?"
            });
            ui.horizontal(|ui| {
                if ui.button("Überschreiben").clicked() {
                    self.send(lesson.id, summary, ctx);
                }
                if ui.button("Abbrechen").clicked() {
                    self.confirm_overwrite = false;
                }
            });
        } else if ui
            .add_enabled(can_send, egui::Button::new("Zusammenfassung senden"))
            .clicked()
            && let Some(lesson) = lesson
        {
            if lesson.summary.is_some() || lesson.released {
                self.confirm_overwrite = true;
            } else {
                self.send(lesson.id, summary, ctx);
            }
        }
    }

    fn ui_new_lesson(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.form.open {
            if ui.small_button("Neue Stunde anlegen …").clicked() {
                self.form.open = true;
                self.form.class_id = self.classes.first().map(|c| c.id);
            }
            return;
        }
        egui::Grid::new("new_lesson").num_columns(2).show(ui, |ui| {
            ui.label("Klasse");
            let current = self
                .classes
                .iter()
                .find(|c| Some(c.id) == self.form.class_id)
                .map_or("–", |c| c.name.as_str());
            egui::ComboBox::from_id_salt("class")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for class in &self.classes {
                        ui.selectable_value(&mut self.form.class_id, Some(class.id), &class.name);
                    }
                });
            ui.end_row();
            ui.label("Fach");
            ui.text_edit_singleline(&mut self.form.subject);
            ui.end_row();
            ui.label("Thema");
            ui.text_edit_singleline(&mut self.form.topic);
            ui.end_row();
        });
        let valid =
            self.form.class_id.is_some() && !self.form.subject.trim().is_empty() && !self.form.topic.trim().is_empty();
        ui.horizontal(|ui| {
            if ui.add_enabled(valid, egui::Button::new("Anlegen")).clicked()
                && let Some(class_id) = self.form.class_id
            {
                let lesson = NewLesson {
                    class_id,
                    subject: self.form.subject.trim().chars().take(255).collect(),
                    topic: self.form.topic.trim().chars().take(255).collect(),
                    held_on: Local::now().date_naive(),
                };
                self.client_job(
                    "Lege Stunde an …",
                    ctx,
                    move |c| c.create_lesson(&lesson),
                    Event::Created,
                );
            }
            if ui.button("Abbrechen").clicked() {
                self.form = NewLessonForm::default();
            }
        });
    }

    fn send(&mut self, lesson_id: u64, summary: &str, ctx: &egui::Context) {
        let summary = summary.to_string();
        self.confirm_overwrite = false;
        self.client_job(
            "Sende Zusammenfassung …",
            ctx,
            move |c| c.set_summary(lesson_id, &summary),
            Event::Sent,
        );
    }
}
