//! Anbindung an die Lernplattform Tuto: verbinden ([`auth`]), Token sicher ablegen ([`store`]) und die
//! Zusammenfassung einer Stunde setzen ([`Client`]).
//!
//! Schnittstelle laut Tuto-Anleitung „App anbinden“: `<Basis-URL>/api/v1`, nur JSON, Bearer-Token mit den
//! Rechten `lessons:read`, `lessons:create`, `lessons:summary`. Daten von Schüler:innen werden weder
//! geliefert noch gesendet.

pub mod auth;
pub mod cli;
pub mod store;

use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fmt;
use std::time::Duration;
use ureq::http::Response;

/// Zugang nach erfolgreichem Verbinden. Das Token gilt 180 Tage.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub base_url: String,
    pub token: String,
    pub expires_at: String,
    pub user_name: String,
    pub role: String,
}

impl fmt::Debug for Session {
    // Das Token darf nie in Logs landen.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("base_url", &self.base_url)
            .field("token", &"…")
            .field("expires_at", &self.expires_at)
            .field("user_name", &self.user_name)
            .field("role", &self.role)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SchoolClass {
    pub id: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Lesson {
    pub id: u64,
    pub subject: String,
    pub topic: String,
    pub held_on: NaiveDate,
    pub class: SchoolClass,
    pub summary: Option<String>,
    #[serde(default)]
    pub summary_from_app: bool,
    /// Hausaufgabe ist schon bei den Schüler:innen – eine neue Zusammenfassung sehen sie sofort.
    #[serde(default)]
    pub released: bool,
}

#[derive(Clone, Debug)]
pub struct NewLesson {
    pub class_id: u64,
    pub subject: String,
    pub topic: String,
    /// Immer das lokale Datum senden: Der Server rechnet in UTC.
    pub held_on: NaiveDate,
}

/// Fehler der Tuto-Schnittstelle. Die Texte von Tuto sind deutsch und für die Lehrkraft gedacht.
#[derive(Debug, PartialEq, Eq)]
pub enum ApiError {
    /// 401: Token abgelaufen oder entzogen → Token löschen und neu verbinden.
    Unauthorized,
    /// 403: Konto darf gerade nicht oder Stunde gehört nicht der Lehrkraft → Meldung zeigen, Token behalten.
    Forbidden(String),
    /// 404: Stunde oder Klasse gibt es nicht (mehr) → Liste neu laden.
    NotFound(String),
    /// 422: Eingabe ungültig, mit den Meldungen je Feld.
    Invalid(String),
    /// 429: zu viele Versuche → eine Minute warten.
    RateLimited,
    Other(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized => {
                f.write_str("Die Verbindung zu Tuto ist abgelaufen oder wurde entzogen. Bitte neu verbinden.")
            }
            Self::RateLimited => f.write_str("Zu viele Versuche. Bitte eine Minute warten."),
            Self::Forbidden(m) | Self::NotFound(m) | Self::Invalid(m) | Self::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ApiError {}

/// HTTP-Client mit kurzen Timeouts. Statuscodes ≥ 400 sind keine ureq-Fehler, damit die Meldung von Tuto
/// gelesen werden kann.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .new_agent()
}

/// Wandelt eine Fehlerantwort in [`ApiError`] um.
fn read_error(resp: &mut Response<ureq::Body>) -> ApiError {
    #[derive(Deserialize, Default)]
    struct Body {
        message: Option<String>,
        #[serde(default)]
        errors: std::collections::BTreeMap<String, Vec<String>>,
    }
    let status = resp.status().as_u16();
    let body: Body = resp.body_mut().read_json().unwrap_or_default();
    let message = body
        .message
        .unwrap_or_else(|| format!("Tuto antwortet mit Fehler {status}"));
    match status {
        401 => ApiError::Unauthorized,
        403 => ApiError::Forbidden(message),
        404 => ApiError::NotFound(message),
        422 => {
            let details: Vec<String> = body.errors.into_values().flatten().collect();
            ApiError::Invalid(if details.is_empty() { message } else { details.join(" ") })
        }
        429 => ApiError::RateLimited,
        _ => ApiError::Other(message),
    }
}

/// Zugriff auf die Stunden der verbundenen Lehrkraft.
pub struct Client {
    session: Session,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(session: Session) -> Self {
        Self {
            session,
            agent: agent(),
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1{path}", self.session.base_url)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.session.token)
    }

    /// Prüft den Status und liest die Antwort als JSON.
    fn json<T: serde::de::DeserializeOwned>(&self, resp: Result<Response<ureq::Body>, ureq::Error>) -> Result<T> {
        let mut resp = resp.with_context(|| format!("Tuto nicht erreichbar unter {}", self.session.base_url))?;
        if !resp.status().is_success() {
            return Err(read_error(&mut resp).into());
        }
        resp.body_mut().read_json().context("Unerwartete Antwort von Tuto")
    }

    /// Fächer, die an der Schule in Stunden vorkommen (alphabetisch). Beim Anlegen bleibt das Fach Freitext.
    pub fn subjects(&self) -> Result<Vec<String>> {
        let resp = self
            .agent
            .get(self.url("/subjects"))
            .header("Accept", "application/json")
            .header("Authorization", self.auth())
            .call();
        Ok(self.json::<Data<Vec<String>>>(resp)?.data)
    }

    /// Namen der Schüler:innen in den Klassen der Lehrkraft (inkl. aktueller Vertretung), nur für die lokale
    /// Pseudonymisierung. `None`, wenn Tuto das (noch) nicht anbietet oder das Token das Recht `roster:read`
    /// nicht hat – dann gilt nur die lokale Klassenliste.
    pub fn roster(&self) -> Result<Option<Vec<String>>> {
        #[derive(Deserialize)]
        struct ClassRoster {
            students: Vec<String>,
        }
        let resp = self
            .agent
            .get(self.url("/roster"))
            .header("Accept", "application/json")
            .header("Authorization", self.auth())
            .call();
        match self.json::<Data<Vec<ClassRoster>>>(resp) {
            Ok(data) => Ok(Some(data.data.into_iter().flat_map(|c| c.students).collect())),
            Err(e)
                if matches!(
                    e.downcast_ref::<ApiError>(),
                    Some(ApiError::Forbidden(_) | ApiError::NotFound(_))
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Klassen, in denen die Lehrkraft Stunden anlegen darf.
    pub fn classes(&self) -> Result<Vec<SchoolClass>> {
        #[derive(Deserialize)]
        struct List {
            data: Vec<SchoolClass>,
        }
        let resp = self
            .agent
            .get(self.url("/classes"))
            .header("Accept", "application/json")
            .header("Authorization", self.auth())
            .call();
        Ok(self.json::<List>(resp)?.data)
    }

    /// Stunden der Lehrkraft an einem Tag (alle Seiten).
    pub fn lessons_on(&self, day: NaiveDate) -> Result<Vec<Lesson>> {
        #[derive(Deserialize)]
        struct Page {
            data: Vec<Lesson>,
            meta: Meta,
        }
        #[derive(Deserialize)]
        struct Meta {
            current_page: u32,
            last_page: u32,
        }
        let mut lessons = Vec::new();
        let mut page = 1u32;
        loop {
            let resp = self
                .agent
                .get(self.url("/lessons"))
                .query("held_on", day.format("%Y-%m-%d").to_string())
                .query("page", page.to_string())
                .header("Accept", "application/json")
                .header("Authorization", self.auth())
                .call();
            let p: Page = self.json(resp)?;
            lessons.extend(p.data);
            // Obergrenze gegen Endlosschleifen bei einer fehlerhaften Antwort.
            if p.meta.current_page >= p.meta.last_page || page >= 20 {
                return Ok(lessons);
            }
            page += 1;
        }
    }

    pub fn create_lesson(&self, lesson: &NewLesson) -> Result<Lesson> {
        let resp = self
            .agent
            .post(self.url("/lessons"))
            .header("Accept", "application/json")
            .header("Authorization", self.auth())
            .send_json(json!({
                "school_class_id": lesson.class_id,
                "subject": lesson.subject,
                "topic": lesson.topic,
                "held_on": lesson.held_on.format("%Y-%m-%d").to_string(),
            }));
        Ok(self.json::<Data<Lesson>>(resp)?.data)
    }

    /// Setzt die Zusammenfassung einer Stunde. Die Länge prüft Tuto selbst (Meldung kommt als 422 zurück).
    pub fn set_summary(&self, lesson_id: u64, summary: &str) -> Result<Lesson> {
        let resp = self
            .agent
            .put(self.url(&format!("/lessons/{lesson_id}/summary")))
            .header("Accept", "application/json")
            .header("Authorization", self.auth())
            .send_json(json!({ "summary": summary }));
        Ok(self.json::<Data<Lesson>>(resp)?.data)
    }

    /// Meldet die App bei Tuto ab (Token wird serverseitig ungültig).
    pub fn logout(&self) -> Result<()> {
        let mut resp = self
            .agent
            .delete(self.url("/token"))
            .header("Accept", "application/json")
            .header("Authorization", self.auth())
            .call()
            .with_context(|| format!("Tuto nicht erreichbar unter {}", self.session.base_url))?;
        // 401: Token war ohnehin schon ungültig – Ziel erreicht.
        if resp.status().is_success() || resp.status().as_u16() == 401 {
            Ok(())
        } else {
            Err(read_error(&mut resp).into())
        }
    }
}

#[derive(Deserialize)]
struct Data<T> {
    data: T,
}

/// Name, unter dem die Lehrkraft die App in Tuto unter „Verbundene Apps“ sieht.
pub fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty());
    match host {
        Some(host) => format!("Tuto Mic auf {host}"),
        None => "Tuto Mic (Desktop)".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_http::serve_once;

    const LESSON: &str = r#"{"id":12,"subject":"Mathematik","topic":"Satz des Pythagoras","held_on":"2026-10-23","class":{"id":3,"name":"7b"},"summary":null,"summary_from_app":false,"released":false}"#;

    fn client(url: &str) -> Client {
        Client::new(Session {
            base_url: url.into(),
            token: "7|geheim".into(),
            expires_at: "2027-04-23T10:15:00+00:00".into(),
            user_name: "Frau Schulze".into(),
            role: "teacher".into(),
        })
    }

    #[test]
    fn debug_hides_token() {
        let debug = format!("{:?}", client("http://x").session);
        assert!(!debug.contains("geheim"), "{debug}");
    }

    #[test]
    fn lists_lessons_of_a_day() {
        let (url, server) = serve_once(
            200,
            format!(
                r#"{{"data":[{LESSON}],"links":{{}},"meta":{{"current_page":1,"last_page":1,"per_page":15,"total":1}}}}"#
            ),
        );
        let day = NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
        let lessons = client(&url).lessons_on(day).unwrap();
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].class.name, "7b");
        assert_eq!(lessons[0].summary, None);

        let req = server.join().unwrap();
        assert!(
            req.head.starts_with("GET /api/v1/lessons?held_on=2026-10-23&page=1 "),
            "{}",
            req.head
        );
        assert!(
            req.head.to_lowercase().contains("authorization: bearer 7|geheim"),
            "{}",
            req.head
        );
        assert!(
            req.head.to_lowercase().contains("accept: application/json"),
            "{}",
            req.head
        );
    }

    #[test]
    fn creates_lesson_with_local_date() {
        let (url, server) = serve_once(201, format!(r#"{{"data":{LESSON}}}"#));
        let lesson = client(&url)
            .create_lesson(&NewLesson {
                class_id: 3,
                subject: "Mathematik".into(),
                topic: "Satz des Pythagoras".into(),
                held_on: NaiveDate::from_ymd_opt(2026, 10, 23).unwrap(),
            })
            .unwrap();
        assert_eq!(lesson.id, 12);
        let body: serde_json::Value = serde_json::from_str(&server.join().unwrap().body).unwrap();
        assert_eq!(body["school_class_id"], 3);
        assert_eq!(body["held_on"], "2026-10-23");
    }

    #[test]
    fn sets_summary() {
        let (url, server) = serve_once(200, format!(r#"{{"data":{LESSON}}}"#));
        client(&url).set_summary(12, "# Mathe\nInhalt").unwrap();
        let req = server.join().unwrap();
        assert!(req.head.starts_with("PUT /api/v1/lessons/12/summary "), "{}", req.head);
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["summary"], "# Mathe\nInhalt");
    }

    #[test]
    fn maps_error_statuses() {
        for (status, body, expected) in [
            (401, r#"{"message":"Unauthenticated."}"#, ApiError::Unauthorized),
            (
                403,
                r#"{"message":"Diese Stunde gehört dir nicht."}"#,
                ApiError::Forbidden("Diese Stunde gehört dir nicht.".into()),
            ),
            (
                422,
                r#"{"message":"Ungültig","errors":{"summary":["Die Zusammenfassung ist zu lang."]}}"#,
                ApiError::Invalid("Die Zusammenfassung ist zu lang.".into()),
            ),
            (429, "", ApiError::RateLimited),
            (500, "kaputt", ApiError::Other("Tuto antwortet mit Fehler 500".into())),
        ] {
            let (url, _server) = serve_once(status, body);
            let err = client(&url).classes().unwrap_err();
            assert_eq!(err.downcast_ref::<ApiError>(), Some(&expected), "Status {status}");
        }
    }

    #[test]
    fn logout_accepts_already_invalid_token() {
        let (url, _server) = serve_once(401, r#"{"message":"Unauthenticated."}"#);
        client(&url).logout().unwrap();
    }

    #[test]
    fn lists_subjects() {
        let (url, server) = serve_once(200, r#"{"data":["Biologie","Mathe"]}"#);
        assert_eq!(client(&url).subjects().unwrap(), ["Biologie", "Mathe"]);
        assert!(server.join().unwrap().head.starts_with("GET /api/v1/subjects "));
    }

    #[test]
    fn roster_flattens_classes() {
        let (url, _server) = serve_once(
            200,
            r#"{"data":[{"class":{"id":3,"name":"7b"},"students":["Lena Wagner","Tom Becker"]},{"class":{"id":4,"name":"7a"},"students":["Jörg Özdemir"]}]}"#,
        );
        assert_eq!(
            client(&url).roster().unwrap(),
            Some(vec![
                "Lena Wagner".to_string(),
                "Tom Becker".into(),
                "Jörg Özdemir".into()
            ])
        );
    }

    #[test]
    fn roster_unavailable_is_not_an_error() {
        for status in [403, 404] {
            let (url, _server) = serve_once(status, r#"{"message":"nein"}"#);
            assert_eq!(client(&url).roster().unwrap(), None, "Status {status}");
        }
        let (url, _server) = serve_once(401, r#"{"message":"Unauthenticated."}"#);
        assert!(client(&url).roster().is_err());
    }

    #[test]
    fn device_name_is_not_empty() {
        assert!(device_name().starts_with("Tuto Mic"));
    }
}
