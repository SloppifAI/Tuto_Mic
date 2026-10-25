//! GUI-Teil der Tuto-Anbindung: Kontostatus in der Kopfzeile ([`TutoPanel::ui_account`]) und das Senden der
//! Zusammenfassung an eine Stunde ([`TutoPanel::ui_send`]).
//!
//! Netzwerkaufrufe laufen in Hintergrund-Threads; die Ergebnisse kommen als [`Event`] zurück, damit die
//! Oberfläche nie blockiert.

use crate::config::Config;
use crate::deeplink;
use crate::theme::{self, BLUE_50, EMERALD_600, ROSE_600};
use crate::tuto::auth::{Callback, PendingLogin};
use crate::tuto::{self, ApiError, Client, Lesson, NewLesson, SchoolClass, Session, TimetableEntry, store};
use chrono::Local;
use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

enum Event {
    Connected(Session),
    Loaded {
        lessons: Vec<Lesson>,
        classes: Vec<SchoolClass>,
        subjects: Vec<String>,
        timetable: Option<Vec<TimetableEntry>>,
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
    /// Fach frei eingeben statt aus der Liste wählen.
    custom_subject: bool,
    topic: String,
}

/// Gängige Schulfächer als Vorschläge. In Tuto ist das Fach Freitext, daher bleibt „Anderes Fach …“ möglich.
const COMMON_SUBJECTS: &[&str] = &[
    "Deutsch",
    "Mathematik",
    "Englisch",
    "Französisch",
    "Latein",
    "Spanisch",
    "Biologie",
    "Chemie",
    "Physik",
    "Geschichte",
    "Erdkunde",
    "Politik",
    "Wirtschaft",
    "Religion",
    "Ethik",
    "Kunst",
    "Musik",
    "Sport",
    "Informatik",
];

/// Stunden von heute aus Tuto.
#[derive(PartialEq, Eq)]
enum LessonsState {
    Needed,
    Loading,
    Loaded,
    /// Fehlgeschlagen: anzeigen statt im nächsten Frame erneut anzufragen.
    Failed(String),
}

pub struct TutoPanel {
    session: Option<Session>,
    login: Option<PendingLogin>,
    paste: String,
    links: Option<Receiver<String>>,
    events: (Sender<Event>, Receiver<Event>),
    busy: Option<String>,
    lessons: Vec<Lesson>,
    classes: Vec<SchoolClass>,
    /// Fächer aus Tuto für die Auswahl beim Anlegen.
    subjects: Vec<String>,
    lessons_state: LessonsState,
    selected: Option<u64>,
    form: NewLessonForm,
    confirm_overwrite: bool,
    message: Option<(bool, String)>,
    /// Warum Rücksprünge über `tutoapp://` nicht ankommen können (z. B. Port durch eine zweite App belegt).
    listen_error: Option<String>,
    /// Erfolgreich gesendete Stunde, bis die App sie mit [`TutoPanel::take_sent`] abholt.
    sent: Option<Lesson>,
    /// Stundenplan heute; `None`, solange Tuto ihn nicht anbietet.
    timetable: Option<Vec<TimetableEntry>>,
    /// Vor der Aufnahme gewählte Stunde.
    plan: Plan,
    /// Die Wahl aus `plan` wurde beim Senden schon übernommen.
    plan_applied: bool,
}

impl TutoPanel {
    /// Startet den Empfang für `tutoapp://`-Rücksprünge und liest einen gespeicherten Zugang.
    pub fn connect() -> Self {
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
            links: Some(links),
            listen_error,
            ..Self::offline()
        }
    }

    /// Liefert einmalig die Stunde, an die gerade erfolgreich gesendet wurde.
    /// Zugang für Hintergrundaufgaben der App (z. B. Klassenliste vor der Transkription laden).
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn take_sent(&mut self) -> Option<Lesson> {
        let sent = self.sent.take();
        if sent.is_some() {
            // Die App zeigt die Bestätigung selbst an.
            self.message = None;
        }
        sent
    }

    /// Ohne Listener und Schlüsselbund, z. B. für Tests und Bildschirmfotos.
    pub fn offline() -> Self {
        Self {
            session: None,
            login: None,
            paste: String::new(),
            links: None,
            events: mpsc::channel(),
            busy: None,
            lessons: Vec::new(),
            classes: Vec::new(),
            subjects: Vec::new(),
            lessons_state: LessonsState::Needed,
            selected: None,
            form: NewLessonForm::default(),
            confirm_overwrite: false,
            message: None,
            listen_error: None,
            sent: None,
            timetable: None,
            plan: Plan::Undecided,
            plan_applied: false,
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

    /// Einmal pro Frame aufrufen: Rücksprünge und Ergebnisse der Hintergrundaufrufe verarbeiten.
    pub fn update(&mut self, ctx: &egui::Context) {
        let links: Vec<String> = self
            .links
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for link in links {
            // Die App nach vorne holen: Die Lehrkraft kommt aus dem Browser zurück oder hat sie ein zweites Mal gestartet.
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Informational,
            ));
            if link != deeplink::ACTIVATE {
                self.handle_link(&link, ctx);
            }
        }
        while let Ok(event) = self.events.1.try_recv() {
            self.busy = None;
            match event {
                Event::Connected(session) => {
                    if let Err(e) = store::save(&session) {
                        self.message = Some((false, format!("{e:#}")));
                    }
                    self.session = Some(session);
                    self.lessons_state = LessonsState::Needed;
                }
                Event::Loaded {
                    lessons,
                    classes,
                    subjects,
                    timetable,
                } => {
                    self.subjects = subjects;
                    if self.plan == Plan::Undecided {
                        let now = Local::now().time();
                        self.plan = timetable
                            .as_deref()
                            .and_then(|entries| current_slot(entries, now))
                            .map_or(Plan::Later, |i| {
                                Plan::Slot(Box::new(timetable.as_ref().expect("oben geprüft")[i].clone()))
                            });
                    }
                    self.timetable = timetable;
                    self.selected = self.selected.filter(|id| lessons.iter().any(|l| l.id == *id));
                    if self.selected.is_none() && lessons.len() == 1 {
                        self.selected = Some(lessons[0].id);
                    }
                    self.lessons = lessons;
                    self.classes = classes;
                    self.lessons_state = LessonsState::Loaded;
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
                            "In Tuto gespeichert – {}, {}, {}",
                            lesson.subject,
                            lesson.class.name,
                            lesson.held_on.format("%d.%m.%Y")
                        ),
                    ));
                    if let Some(l) = self.lessons.iter_mut().find(|l| l.id == lesson.id) {
                        l.clone_from(&lesson);
                    }
                    self.confirm_overwrite = false;
                    self.sent = Some(lesson);
                    // Für die nächste Aufnahme neu vorbelegen.
                    self.plan = Plan::Undecided;
                    self.plan_applied = false;
                    self.lessons_state = LessonsState::Needed;
                }
                Event::Failed(e) if self.lessons_state == LessonsState::Loading => {
                    self.lessons_state = LessonsState::Failed(e);
                }
                Event::Failed(e) => self.message = Some((false, e)),
                Event::Disconnected => {
                    self.lessons_state = LessonsState::Needed;
                    let _ = store::delete();
                    self.session = None;
                    self.message = Some((false, ApiError::Unauthorized.to_string()));
                }
            }
        }
        if self.login.as_ref().is_some_and(PendingLogin::is_expired) {
            self.login = None;
        }
        if self.login.is_some() || self.busy.is_some() {
            ctx.request_repaint_after(Duration::from_millis(300));
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
                self.message = None;
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

    fn disconnect(&mut self) {
        if let Some(session) = self.session.take() {
            // Abmelden bei Tuto im Hintergrund; der lokale Zugang ist sofort weg.
            std::thread::spawn(move || {
                let _ = Client::new(session).logout();
            });
        }
        self.lessons.clear();
        self.lessons_state = LessonsState::Needed;
        self.selected = None;
        if let Err(e) = store::delete() {
            self.message = Some((false, format!("{e:#}")));
        }
    }

    /// Kontostatus für die Kopfzeile: verbinden, Anmeldung läuft, oder verbunden (mit Menü zum Trennen).
    pub fn ui_account(&mut self, ui: &mut egui::Ui, cfg: &Config) {
        if let Some(session) = &self.session {
            let name = session.user_name.clone();
            let detail = format!(
                "{}\nverbunden bis {}",
                session.base_url.trim_start_matches("https://"),
                session.expires_at.get(..10).unwrap_or(&session.expires_at)
            );
            let mut disconnect = false;
            ui.menu_button(name, |ui| {
                ui.label(RichText::new("Mit Tuto verbunden").font(theme::heading(14.0)));
                ui.label(RichText::new(detail).small().color(theme::weak_text(ui)));
                ui.separator();
                if ui.button("Verbindung trennen").clicked() {
                    disconnect = true;
                    ui.close();
                }
            });
            // Grüner Punkt als Zeichen für „verbunden“ (egui-Schriften haben kein ●). Die Kopfzeile ordnet von rechts
            // nach links an, daher steht er nach dem Knopf im Code und davor auf dem Bildschirm.
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 5.0, EMERALD_600);
            if disconnect {
                self.disconnect();
            }
        } else if self.login.is_some() || self.busy.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Anmeldung im Browser …").color(theme::weak_text(ui)));
            });
        } else if ui.button("Mit Tuto verbinden").clicked() {
            self.start_login(cfg);
        }
    }

    /// Bereich „An Tuto senden“ unter der Zusammenfassung.
    pub fn ui_send(&mut self, ui: &mut egui::Ui, cfg: &Config, summary: Option<&str>) {
        let ctx = ui.ctx().clone();
        ui.label(RichText::new("An Tuto senden").font(theme::heading(18.0)));
        ui.add_space(2.0);

        if let Some(label) = &self.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(label);
            });
        } else if self.session.is_none() {
            self.ui_login(ui, cfg, &ctx);
        } else {
            self.ui_lessons(ui, cfg, summary, &ctx);
        }

        if let Some((ok, msg)) = &self.message {
            ui.add_space(4.0);
            ui.label(RichText::new(msg).color(if *ok { EMERALD_600 } else { ROSE_600 }));
        }
    }

    fn ui_login(&mut self, ui: &mut egui::Ui, cfg: &Config, ctx: &egui::Context) {
        let weak = theme::weak_text(ui);
        if self.login.is_none() {
            ui.label(
                RichText::new(
                    "Verbinde Tuto Mic einmalig mit deinem Tuto-Konto. Danach landet die Zusammenfassung mit einem \
                     Klick bei der passenden Stunde.",
                )
                .color(weak),
            );
            if ui.add(theme::primary_button("Mit Tuto verbinden")).clicked() {
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
                .color(ROSE_600),
            );
        }
        ui.collapsing(RichText::new("Kommt nichts zurück?").color(weak), |ui| {
            ui.label(
                RichText::new(
                    "Zeigt der Browser eine Adresse, die mit tutoapp://callback beginnt, kann sie hier eingefügt werden.",
                )
                .small()
                .color(weak),
            );
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.paste)
                        .hint_text("tutoapp://callback?code=…")
                        .desired_width(300.0),
                );
                if ui.add_enabled(!self.paste.trim().is_empty(), egui::Button::new("Übernehmen")).clicked() {
                    let link = std::mem::take(&mut self.paste);
                    self.handle_link(&link, ctx);
                }
            });
        });
        if ui.button("Abbrechen").clicked() {
            self.login = None;
        }
    }

    /// Nach Verwerfen oder Abbruch: Die nächste Aufnahme gehört zu einer anderen Stunde, also Stundenplan neu laden
    /// und nach der Uhrzeit neu vorbelegen statt die alte Wahl stehen zu lassen.
    pub fn reset_plan(&mut self) {
        self.plan = Plan::Undecided;
        self.form = NewLessonForm::default();
        self.plan_applied = false;
        self.selected = None;
        self.lessons_state = LessonsState::Needed;
    }

    /// Stößt das Laden der Stunden an und zeigt solange einen Hinweis. Nach einem Fehler wird erst auf Knopfdruck
    /// erneut gefragt, sonst ginge im Takt der Bildwiederholung eine Anfrage nach der anderen an Tuto.
    /// Liefert `true`, sobald die Stunden da sind.
    fn ensure_loaded(&mut self, ui: &mut egui::Ui) -> bool {
        if self.lessons_state == LessonsState::Loaded {
            return true;
        }
        if let LessonsState::Failed(e) = &self.lessons_state {
            theme::warning(
                ui,
                &format!(
                    "Die Stunden konnten nicht aus Tuto geladen werden ({e}). Bitte die Internetverbindung prüfen."
                ),
            );
            if ui.button("Erneut versuchen").clicked() {
                self.lessons_state = LessonsState::Needed;
            }
            return false;
        }
        if self.busy.is_none() {
            self.load(ui.ctx());
        }
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Stunden werden geladen …").color(theme::weak_text(ui)));
        });
        false
    }

    /// Stunden, Klassen, Fächer und Stundenplan von heute im Hintergrund laden.
    fn load(&mut self, ctx: &egui::Context) {
        let today = Local::now().date_naive();
        self.lessons_state = LessonsState::Loading;
        self.client_job(
            "Lade Stunden …",
            ctx,
            move |c| {
                // Fächer und Stundenplan sind Komfort: Fehlen sie (ältere Tuto-Version), geht es ohne weiter.
                let subjects = c.subjects().unwrap_or_default();
                let timetable = c.timetable(today).unwrap_or_else(|e| {
                    log::warn!("Stundenplan nicht ladbar: {e:#}");
                    None
                });
                Ok((c.lessons_on(today)?, c.classes()?, subjects, timetable))
            },
            |(lessons, classes, subjects, timetable)| Event::Loaded {
                lessons,
                classes,
                subjects,
                timetable,
            },
        );
    }

    /// Klasse und Fach der vor der Aufnahme gewählten Stunde, als Kontext für den Prompt.
    pub fn planned_context(&self) -> Option<String> {
        let (class, subject) = match &self.plan {
            Plan::Slot(entry) => (entry.class.name.clone(), entry.subject.clone()),
            Plan::Lesson(id) => {
                let lesson = self.lessons.iter().find(|l| l.id == *id)?;
                (lesson.class.name.clone(), lesson.subject.clone())
            }
            Plan::New => {
                let class = self.classes.iter().find(|c| Some(c.id) == self.form.class_id)?;
                let subject = self.form.subject.trim();
                if subject.is_empty() {
                    return None;
                }
                (class.name.clone(), subject.to_string())
            }
            Plan::Undecided | Plan::Later => return None,
        };
        Some(format!("Fach: {subject}, Klasse: {class}"))
    }

    /// Auswahl „Für welche Stunde?“ auf der Startseite. Vorbelegt mit der Stunde, die laut Stundenplan gerade läuft.
    pub fn ui_plan(&mut self, ui: &mut egui::Ui) {
        if self.session.is_none() {
            return;
        }
        if !self.ensure_loaded(ui) {
            return;
        }
        let mut options: Vec<(Plan, String)> = Vec::new();
        for entry in self.timetable.iter().flatten() {
            let room = entry.room.as_deref().map(|r| format!(", {r}")).unwrap_or_default();
            options.push((
                Plan::Slot(Box::new(entry.clone())),
                format!(
                    "{}–{}  {} · {}{room}",
                    entry.starts_at.format("%H:%M"),
                    entry.ends_at.format("%H:%M"),
                    entry.class.name,
                    entry.subject
                ),
            ));
        }
        let in_timetable: Vec<u64> = self.timetable.iter().flatten().filter_map(|e| e.lesson_id).collect();
        for lesson in self.lessons.iter().filter(|l| !in_timetable.contains(&l.id)) {
            options.push((
                Plan::Lesson(lesson.id),
                format!("{} · {}: {}", lesson.class.name, lesson.subject, lesson.topic),
            ));
        }
        options.push((Plan::New, "Neue Stunde …".into()));
        options.push((Plan::Later, "Später wählen".into()));
        let current = options
            .iter()
            .find(|(p, _)| *p == self.plan)
            .map_or("Später wählen".to_string(), |(_, label)| label.clone());

        ui.horizontal(|ui| {
            let label = ui.label("Für welche Stunde?");
            egui::ComboBox::from_id_salt("plan")
                .selected_text(current)
                .width(320.0)
                .show_ui(ui, |ui| {
                    for (plan, text) in options {
                        if ui.selectable_label(self.plan == plan, text).clicked() {
                            if plan == Plan::New && !self.form.open {
                                self.form = NewLessonForm {
                                    open: true,
                                    class_id: self.classes.first().map(|c| c.id),
                                    ..NewLessonForm::default()
                                };
                            } else if plan != Plan::New {
                                self.form = NewLessonForm::default();
                            }
                            self.plan = plan;
                            self.plan_applied = false;
                        }
                    }
                })
                .response
                .labelled_by(label.id);
        });
        if self.plan == Plan::New {
            ui.add_space(4.0);
            egui::Grid::new("plan_new_lesson")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    let l = ui.label("Klasse");
                    self.class_picker(ui, l.id);
                    ui.end_row();
                    let l = ui.label("Fach");
                    self.subject_picker(ui, l.id);
                    ui.end_row();
                });
            ui.label(
                RichText::new("Die Stunde wird beim Senden in Tuto angelegt, das Thema kommt aus der Zusammenfassung.")
                    .small()
                    .color(theme::weak_text(ui)),
            );
        }
        if self.timetable.is_none() && self.plan != Plan::New {
            ui.label(
                RichText::new("Ohne Stundenplan in Tuto wird keine Stunde vorgeschlagen.")
                    .small()
                    .color(theme::weak_text(ui)),
            );
        }
    }

    /// Übernimmt die vor der Aufnahme gewählte Stunde einmalig in den Senden-Bereich: vorhandene Stunde wählen oder
    /// das Formular „Neue Stunde“ mit Klasse, Fach und (aus der Zusammenfassung) Thema vorbelegen.
    fn apply_plan(&mut self, summary: Option<&str>) {
        if self.plan_applied {
            return;
        }
        self.plan_applied = true;
        let lesson_id = match &self.plan {
            Plan::Lesson(id) => Some(*id),
            Plan::Slot(entry) => entry.lesson_id,
            Plan::New => {
                // Klasse und Fach stehen schon im Formular; das Thema kommt aus der Zusammenfassung.
                self.form.open = true;
                if self.form.topic.trim().is_empty()
                    && let Some((_, topic)) = summary.and_then(heading_subject_topic)
                {
                    self.form.topic = topic;
                }
                return;
            }
            Plan::Undecided | Plan::Later => return,
        };
        if let Some(id) = lesson_id.filter(|id| self.lessons.iter().any(|l| l.id == *id)) {
            self.selected = Some(id);
            return;
        }
        if let Plan::Slot(entry) = &self.plan {
            self.form = NewLessonForm {
                open: true,
                class_id: Some(entry.class.id),
                subject: entry.subject.clone(),
                custom_subject: false,
                topic: summary
                    .and_then(heading_subject_topic)
                    .map(|(_, topic)| topic)
                    .unwrap_or_default(),
            };
        }
    }

    fn ui_lessons(&mut self, ui: &mut egui::Ui, cfg: &Config, summary: Option<&str>, ctx: &egui::Context) {
        if !self.ensure_loaded(ui) {
            return;
        }
        self.apply_plan(summary);
        let weak = theme::weak_text(ui);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Deine Stunden heute, {}", Local::now().format("%d.%m.%Y"))).color(weak));
            if ui.small_button("Aktualisieren").clicked() {
                self.lessons_state = LessonsState::Needed;
            }
        });
        if self.lessons.is_empty() && !self.form.open {
            ui.label(RichText::new("Für heute ist in Tuto noch keine Stunde angelegt.").color(weak));
        }
        for lesson in &self.lessons {
            let selected = self.selected == Some(lesson.id);
            if lesson_row(ui, lesson, selected).clicked() {
                self.selected = Some(lesson.id);
                self.confirm_overwrite = false;
            }
        }
        self.ui_new_lesson(ui, summary, ctx);

        ui.add_space(8.0);
        let Some(summary) = summary.map(str::trim).filter(|s| !s.is_empty()) else {
            ui.label(RichText::new("Keine Zusammenfassung vorhanden.").color(weak));
            return;
        };
        let too_long = summary.chars().count() > cfg.summary_limit;
        let lesson = self
            .selected
            .and_then(|id| self.lessons.iter().find(|l| l.id == id))
            .cloned();
        if self.confirm_overwrite
            && let Some(lesson) = &lesson
        {
            ui.label(if lesson.released {
                "Die Hausaufgabe ist schon freigegeben – die Schüler:innen sehen die neue Fassung sofort. Überschreiben?"
            } else {
                "Diese Stunde hat schon eine Zusammenfassung. Überschreiben?"
            });
            ui.horizontal(|ui| {
                if ui.add(theme::primary_button("Überschreiben")).clicked() {
                    self.send(lesson.id, summary, ctx);
                }
                if ui.button("Abbrechen").clicked() {
                    self.confirm_overwrite = false;
                }
            });
            return;
        }
        // Schon bei der Auswahl warnen, nicht erst beim Senden.
        if let Some(lesson) = lesson.as_ref().filter(|l| l.summary.is_some() || l.released) {
            theme::warning(
                ui,
                if lesson.released {
                    "Diese Stunde hat schon eine Zusammenfassung und die Hausaufgabe ist freigegeben. Beim Senden wird \
                     sie ersetzt – die Schüler:innen sehen die neue Fassung sofort."
                } else {
                    "Diese Stunde hat schon eine Zusammenfassung. Beim Senden wird sie ersetzt."
                },
            );
        }
        let label = match &lesson {
            Some(_) if too_long => "Zu lang zum Senden",
            Some(l) if l.summary.is_some() || l.released => "Zusammenfassung ersetzen",
            Some(_) => "Zusammenfassung senden",
            None => "Erst eine Stunde wählen",
        };
        if ui
            .add_enabled(lesson.is_some() && !too_long, theme::primary_button(label))
            .clicked()
            && let Some(lesson) = &lesson
        {
            if lesson.summary.is_some() || lesson.released {
                self.confirm_overwrite = true;
            } else {
                self.send(lesson.id, summary, ctx);
            }
        }
    }

    fn ui_new_lesson(&mut self, ui: &mut egui::Ui, summary: Option<&str>, ctx: &egui::Context) {
        if !self.form.open {
            if ui.button("+ Neue Stunde anlegen").clicked() {
                self.form.open = true;
                self.form.class_id = self.classes.first().map(|c| c.id);
                // Fach und Thema aus der Überschrift der Zusammenfassung vorschlagen.
                if let Some((subject, topic)) = summary.and_then(heading_subject_topic) {
                    self.form.subject = subject;
                    self.form.topic = topic;
                }
            }
            return;
        }
        egui::Frame::new()
            .fill(if ui.visuals().dark_mode {
                theme::SLATE_800
            } else {
                BLUE_50
            })
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Neue Stunde für heute").font(theme::heading(14.0)));
                egui::Grid::new("new_lesson")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        let class_label = ui.label("Klasse");
                        self.class_picker(ui, class_label.id);
                        ui.end_row();
                        let l = ui.label("Fach");
                        self.subject_picker(ui, l.id);
                        ui.end_row();
                        let l = ui.label("Thema");
                        ui.add(egui::TextEdit::singleline(&mut self.form.topic).hint_text("z. B. Prozentrechnung"))
                            .labelled_by(l.id);
                        ui.end_row();
                    });
                let valid = self.form.class_id.is_some()
                    && !self.form.subject.trim().is_empty()
                    && !self.form.topic.trim().is_empty();
                ui.horizontal(|ui| {
                    if ui.add_enabled(valid, theme::primary_button("Anlegen")).clicked()
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
            });
    }

    fn class_picker(&mut self, ui: &mut egui::Ui, label: egui::Id) {
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
            })
            .response
            .labelled_by(label);
    }

    /// Fach aus Vorschlägen wählen (gängige Fächer und die aus den eigenen Tuto-Stunden) oder frei eingeben.
    fn subject_picker(&mut self, ui: &mut egui::Ui, label: egui::Id) {
        // Zuerst die Fächer der Schule aus Tuto, dann die der heutigen Stunden, dann gängige Fächer.
        let mut subjects: Vec<String> = self.subjects.clone();
        subjects.extend(self.lessons.iter().map(|l| l.subject.clone()));
        subjects.extend(COMMON_SUBJECTS.iter().map(|s| (*s).to_string()));
        let mut seen = std::collections::HashSet::new();
        subjects.retain(|s| seen.insert(s.to_lowercase()));

        if self.form.custom_subject {
            ui.add(egui::TextEdit::singleline(&mut self.form.subject).hint_text("Fach eingeben"))
                .labelled_by(label);
            return;
        }
        let shown = if self.form.subject.is_empty() {
            "Fach wählen"
        } else {
            self.form.subject.as_str()
        };
        egui::ComboBox::from_id_salt("subject")
            .selected_text(shown)
            .height(320.0)
            .show_ui(ui, |ui| {
                for subject in &subjects {
                    ui.selectable_value(&mut self.form.subject, subject.clone(), subject);
                }
                ui.separator();
                if ui.selectable_label(false, "Anderes Fach …").clicked() {
                    self.form.custom_subject = true;
                    self.form.subject.clear();
                }
            })
            .response
            .labelled_by(label);
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

/// Fach und Thema aus der ersten Überschrift der Zusammenfassung, Format laut Prompt:
/// `# [Fach], Klasse [Stufe]: [Thema der Stunde]`. Fehlt der Doppelpunkt, gibt es keinen Vorschlag.
fn heading_subject_topic(summary: &str) -> Option<(String, String)> {
    let heading = summary
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("# "))?
        .trim_start_matches("# ");
    let (head, topic) = heading.split_once(':')?;
    let subject = head.split(',').next()?.trim();
    let topic = topic.trim();
    (!subject.is_empty() && !topic.is_empty()).then(|| (subject.to_string(), topic.to_string()))
}

/// Vor der Aufnahme gewählte Stunde.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Plan {
    /// Noch nichts gewählt; nach dem Laden wird vorbelegt.
    Undecided,
    /// Eintrag aus dem Stundenplan (mit oder ohne schon angelegte Stunde).
    Slot(Box<TimetableEntry>),
    /// Stunde aus Tuto ohne Stundenplan-Eintrag.
    Lesson(u64),
    /// Neue Stunde, Klasse und Fach stehen in `TutoPanel::form`. Angelegt wird sie erst beim Senden, damit bei
    /// verworfener Aufnahme keine leere Stunde in Tuto zurückbleibt.
    New,
    Later,
}

/// Die Stunde, die zu `now` läuft – mit 10 Minuten Vorlauf, damit vor Beginn gestartete Aufnahmen passen. Läuft
/// keine, die nächste innerhalb von 15 Minuten.
fn current_slot(entries: &[TimetableEntry], now: chrono::NaiveTime) -> Option<usize> {
    let lead = chrono::TimeDelta::minutes(10);
    entries
        .iter()
        .position(|e| e.starts_at - lead <= now && now <= e.ends_at)
        .or_else(|| {
            entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.starts_at > now && e.starts_at - now <= chrono::TimeDelta::minutes(15))
                .min_by_key(|(_, e)| e.starts_at)
                .map(|(i, _)| i)
        })
}

/// Auswählbare Zeile für eine Stunde: Klasse, Fach, Thema und Hinweis auf eine vorhandene Zusammenfassung.
fn lesson_row(ui: &mut egui::Ui, lesson: &Lesson, selected: bool) -> egui::Response {
    let dark = ui.visuals().dark_mode;
    let (fill, stroke) = match (selected, dark) {
        (true, false) => (BLUE_50, theme::BLUE_600),
        (true, true) => (theme::SLATE_800, theme::BLUE_400),
        (false, false) => (Color32::WHITE, theme::SLATE_200),
        (false, true) => (theme::SLATE_900, theme::SLATE_800),
    };
    let response = egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(if selected { 2.0 } else { 1.0 }, stroke))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(&lesson.class.name).font(theme::heading(15.0)));
                ui.label(format!("{}: {}", lesson.subject, lesson.topic));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let amber = Color32::from_rgb(0xff, 0xf7, 0xe6);
                    if lesson.released {
                        theme::badge(ui, "Hausaufgabe freigegeben", theme::AMBER_700, amber);
                    }
                    if lesson.summary.is_some() {
                        theme::badge(ui, "hat Zusammenfassung", theme::AMBER_700, amber);
                    }
                });
            });
        })
        .response;
    response
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
impl TutoPanel {
    /// Wie [`Self::demo_connected`], mit geöffnetem Formular „Neue Stunde“.
    pub fn demo_new_lesson(lessons: Vec<Lesson>) -> Self {
        let mut panel = Self::demo_connected(lessons, None);
        panel.form = NewLessonForm {
            open: true,
            class_id: Some(3),
            subject: "Mathematik".into(),
            custom_subject: false,
            topic: String::new(),
        };
        panel
    }

    /// Startseite mit „Neue Stunde …“ als Wahl vor der Aufnahme.
    pub fn demo_plan_new_lesson() -> Self {
        let mut panel = Self::demo_new_lesson(Vec::new());
        panel.plan = Plan::New;
        panel
    }

    /// Verbunden, mit Stundenplan; vorbelegt ist der Eintrag `selected`.
    pub fn demo_timetable(selected: usize) -> Self {
        let class = SchoolClass {
            id: 3,
            name: "7b".into(),
        };
        let slot = |h: u32, m: u32, eh: u32, em: u32, subject: &str, room: &str| TimetableEntry {
            starts_at: chrono::NaiveTime::from_hms_opt(h, m, 0).unwrap(),
            ends_at: chrono::NaiveTime::from_hms_opt(eh, em, 0).unwrap(),
            class: class.clone(),
            subject: subject.into(),
            room: Some(room.into()),
            lesson_id: None,
        };
        let entries = vec![
            slot(8, 0, 8, 45, "Deutsch", "B12"),
            slot(8, 50, 9, 35, "Mathematik", "B12"),
        ];
        let mut panel = Self::demo_connected(Vec::new(), None);
        panel.plan = Plan::Slot(Box::new(entries[selected].clone()));
        panel.timetable = Some(entries);
        panel
    }

    /// Zustand „verbunden, Stunden geladen“ für Bildschirmfotos.
    pub fn demo_connected(lessons: Vec<Lesson>, selected: Option<u64>) -> Self {
        Self {
            session: Some(Session {
                base_url: "https://tuto.example".into(),
                token: "test".into(),
                expires_at: "2027-04-23T10:15:00+00:00".into(),
                user_name: "Frau Schulze".into(),
                role: "teacher".into(),
            }),
            lessons,
            classes: vec![SchoolClass {
                id: 3,
                name: "7b".into(),
            }],
            lessons_state: LessonsState::Loaded,
            selected,
            ..Self::offline()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_and_topic_from_heading() {
        let summary = "# Mathematik, Klasse 7: Prozentrechnung mit eigenen Daten der Klasse\n\n## Lernziele\n- …";
        assert_eq!(
            heading_subject_topic(summary),
            Some((
                "Mathematik".into(),
                "Prozentrechnung mit eigenen Daten der Klasse".into()
            ))
        );
        assert_eq!(
            heading_subject_topic("\n# Geschichte: Weimarer Republik\n"),
            Some(("Geschichte".into(), "Weimarer Republik".into()))
        );
        assert_eq!(heading_subject_topic("## Lernziele\n- …"), None);
        assert_eq!(heading_subject_topic("# Nur eine Überschrift"), None);
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use chrono::NaiveTime;

    fn entry(start: (u32, u32), end: (u32, u32), class: &str) -> TimetableEntry {
        TimetableEntry {
            starts_at: NaiveTime::from_hms_opt(start.0, start.1, 0).unwrap(),
            ends_at: NaiveTime::from_hms_opt(end.0, end.1, 0).unwrap(),
            class: SchoolClass {
                id: 1,
                name: class.into(),
            },
            subject: "Mathematik".into(),
            room: None,
            lesson_id: None,
        }
    }

    fn at(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn preselects_running_or_upcoming_lesson() {
        let day = [
            entry((8, 0), (8, 45), "7a"),
            entry((8, 50), (9, 35), "7b"),
            entry((11, 40), (12, 25), "8c"),
        ];
        assert_eq!(current_slot(&day, at(8, 10)), Some(0), "läuft gerade");
        assert_eq!(
            current_slot(&day, at(8, 47)),
            Some(1),
            "Pause: 10 min Vorlauf der nächsten Stunde"
        );
        assert_eq!(current_slot(&day, at(11, 28)), Some(2), "beginnt in 12 min");
        assert_eq!(current_slot(&day, at(10, 30)), None, "Freistunde");
        assert_eq!(current_slot(&day, at(13, 0)), None, "Schulschluss");
        assert_eq!(current_slot(&[], at(9, 0)), None);
    }

    #[test]
    fn planned_slot_gives_prompt_context() {
        let mut panel = TutoPanel::offline();
        assert_eq!(panel.planned_context(), None);
        panel.plan = Plan::Slot(Box::new(entry((8, 0), (8, 45), "7b")));
        assert_eq!(panel.planned_context().as_deref(), Some("Fach: Mathematik, Klasse: 7b"));
        panel.plan = Plan::Later;
        assert_eq!(panel.planned_context(), None);
    }

    #[test]
    fn new_lesson_planned_before_recording() {
        let mut panel = TutoPanel::demo_new_lesson(Vec::new());
        panel.plan = Plan::New;
        assert_eq!(panel.planned_context().as_deref(), Some("Fach: Mathematik, Klasse: 7b"));

        // Beim Senden: Formular offen, Thema aus der Zusammenfassung, angelegt ist noch nichts.
        panel.form.open = false;
        panel.apply_plan(Some("# Mathematik, Klasse 7: Prozentrechnung\n- …"));
        assert!(panel.form.open);
        assert_eq!(panel.form.topic, "Prozentrechnung");
        assert_eq!(panel.selected, None);

        panel.form.subject.clear();
        assert_eq!(panel.planned_context(), None, "ohne Fach kein halber Kontext");
        panel.reset_plan();
        assert_eq!((panel.plan, panel.form.open), (Plan::Undecided, false));
    }
}
