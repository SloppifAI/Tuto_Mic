//! Verbinden mit Tuto: OAuth 2.0 „Authorization Code“ mit PKCE (RFC 7636) im System-Browser.
//!
//! Ablauf: [`PendingLogin::start`] erzeugt Prüfwerte und die Adresse für den Browser. Tuto leitet nach
//! „Erlauben“ auf `tutoapp://callback?code=…&state=…` zurück; [`PendingLogin::finish`] prüft den Rücksprung
//! und tauscht den Code gegen ein Token. Die App sieht dabei nie ein Passwort.

use super::{ApiError, Session, read_error};
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::time::{Duration, Instant};
use url::Url;

pub const CLIENT_ID: &str = "tuto-app";
/// Muss Zeichen für Zeichen der in Tuto registrierten Adresse entsprechen.
pub const REDIRECT_URI: &str = "tutoapp://callback";
/// So lange gilt der Code, den Tuto nach „Erlauben“ ausstellt.
const CODE_LIFETIME: Duration = Duration::from_mins(5);

/// Ein begonnener Verbindungsversuch. Hält Verifier und `state` bis zum Rücksprung im Speicher.
pub struct PendingLogin {
    base_url: String,
    verifier: String,
    state: String,
    started: Instant,
    /// Adresse, die im Browser geöffnet wird.
    pub authorize_url: String,
}

/// Ergebnis eines Rücksprungs, der zu diesem Verbindungsversuch gehört.
#[derive(Debug, PartialEq, Eq)]
pub enum Callback {
    Code(String),
    Denied,
}

impl PendingLogin {
    pub fn start(base_url: &str) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_string();
        let verifier = random_token(32)?;
        let state = random_token(16)?;
        let authorize_url = authorize_url(&base_url, &challenge(&verifier), &state)?;
        Ok(Self {
            base_url,
            verifier,
            state,
            started: Instant::now(),
            authorize_url,
        })
    }

    pub fn is_expired(&self) -> bool {
        // Der Code gilt ab „Erlauben“; großzügig gerechnet ab Beginn plus Zeit für die Anmeldung.
        self.started.elapsed() > CODE_LIFETIME * 3
    }

    /// Prüft eine Rücksprung-Adresse. Fremde Adressen oder ein anderer `state` sind ein Fehler.
    pub fn parse_callback(&self, callback: &str) -> Result<Callback> {
        parse_callback(callback, &self.state)
    }

    /// Tauscht den Code gegen ein Token. Jeder Versuch verbraucht den Code – nach einem Fehler muss die
    /// Verbindung neu beginnen.
    pub fn finish(&self, code: &str, device_name: &str) -> Result<Session> {
        #[derive(Deserialize)]
        struct TokenResponse {
            token: String,
            expires_at: String,
            user: User,
        }
        #[derive(Deserialize)]
        struct User {
            name: String,
            role: String,
        }

        let mut resp = super::agent()
            .post(format!("{}/api/v1/token", self.base_url))
            .header("Accept", "application/json")
            .send_json(json!({
                "grant_type": "authorization_code",
                "code": code,
                "client_id": CLIENT_ID,
                "redirect_uri": REDIRECT_URI,
                "code_verifier": self.verifier,
                "device_name": device_name.chars().take(100).collect::<String>(),
            }))
            .with_context(|| format!("Tuto nicht erreichbar unter {}", self.base_url))?;
        if !resp.status().is_success() {
            return Err(read_error(&mut resp).into());
        }
        let body: TokenResponse = resp.body_mut().read_json().context("Unerwartete Antwort von Tuto")?;
        Ok(Session {
            base_url: self.base_url.clone(),
            token: body.token,
            expires_at: body.expires_at,
            user_name: body.user.name,
            role: body.user.role,
        })
    }
}

/// Zufallswert, `Base64URL` ohne Auffüllung (32 Bytes → 43 Zeichen, wie RFC 7636 empfiehlt).
fn random_token(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| anyhow::anyhow!("Zufallszahlen nicht verfügbar: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// `code_challenge` = BASE64URL(SHA-256(verifier)).
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn authorize_url(base_url: &str, challenge: &str, state: &str) -> Result<String> {
    let mut url = Url::parse(&format!("{base_url}/app/verbinden"))
        .with_context(|| format!("Ungültige Tuto-Adresse: {base_url}"))?;
    url.query_pairs_mut()
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state);
    Ok(url.into())
}

fn parse_callback(callback: &str, expected_state: &str) -> Result<Callback> {
    let url = Url::parse(callback.trim()).context("Das ist keine gültige Adresse")?;
    if url.scheme() != "tutoapp" || url.host_str() != Some("callback") {
        bail!("Die Adresse muss mit {REDIRECT_URI} beginnen");
    }
    let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
    // Zuerst `state` prüfen: Ein fremder Rücksprung wird verworfen, egal was er sonst enthält.
    if param("state").as_deref() != Some(expected_state) {
        bail!("Die Rückmeldung gehört nicht zu diesem Verbindungsversuch. Bitte noch einmal verbinden.");
    }
    match (param("code"), param("error")) {
        (Some(code), _) if !code.is_empty() => Ok(Callback::Code(code)),
        (_, Some(error)) if error == "access_denied" => Ok(Callback::Denied),
        (_, Some(error)) => Err(ApiError::Other(format!("Tuto meldet: {error}")).into()),
        _ => bail!("Die Rückmeldung von Tuto enthält keinen Code"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_http::serve_once;

    #[test]
    fn challenge_matches_rfc_7636_example() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_values_have_expected_shape() {
        let a = random_token(32).unwrap();
        assert_eq!(a.len(), 43);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(a, random_token(32).unwrap());
    }

    #[test]
    fn builds_authorize_url() {
        let login = PendingLogin::start("https://tuto.example/").unwrap();
        let url = Url::parse(&login.authorize_url).unwrap();
        assert_eq!(url.path(), "/app/verbinden");
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "tuto-app");
        assert_eq!(q["redirect_uri"], "tutoapp://callback");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], challenge(&login.verifier));
        assert_eq!(q["state"], login.state);
        assert!(login.authorize_url.contains("redirect_uri=tutoapp%3A%2F%2Fcallback"));
    }

    #[test]
    fn parses_callbacks() {
        assert_eq!(
            parse_callback("tutoapp://callback?code=abc&state=s1", "s1").unwrap(),
            Callback::Code("abc".into())
        );
        assert_eq!(
            parse_callback(" tutoapp://callback?error=access_denied&state=s1\n", "s1").unwrap(),
            Callback::Denied
        );
    }

    #[test]
    fn rejects_foreign_or_broken_callbacks() {
        assert!(parse_callback("tutoapp://callback?code=abc&state=anders", "s1").is_err());
        assert!(parse_callback("tutoapp://callback?code=abc", "s1").is_err());
        assert!(parse_callback("https://evil.example/callback?code=abc&state=s1", "s1").is_err());
        assert!(parse_callback("tutoapp://callback?state=s1", "s1").is_err());
        assert!(parse_callback("kein link", "s1").is_err());
    }

    #[test]
    fn exchanges_code_for_token() {
        let (url, server) = serve_once(
            201,
            r#"{"token":"7|geheim","token_type":"Bearer","abilities":[],"expires_at":"2027-04-23T10:15:00+00:00","user":{"name":"Frau Schulze","role":"teacher"}}"#,
        );
        let login = PendingLogin::start(&url).unwrap();
        let session = login.finish("code123", "Tuto Mic auf Testrechner").unwrap();
        assert_eq!(session.token, "7|geheim");
        assert_eq!(session.user_name, "Frau Schulze");
        assert_eq!(session.base_url, url);

        let req = server.join().unwrap();
        assert!(req.head.starts_with("POST /api/v1/token "), "{}", req.head);
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["code"], "code123");
        assert_eq!(body["code_verifier"], login.verifier);
        assert_eq!(body["redirect_uri"], REDIRECT_URI);
        assert_eq!(body["client_id"], CLIENT_ID);
    }

    #[test]
    fn shows_tuto_error_message() {
        let (url, _server) = serve_once(
            400,
            r#"{"error":"invalid_grant","message":"Der Code ist ungültig, abgelaufen oder schon benutzt."}"#,
        );
        let err = PendingLogin::start(&url).unwrap().finish("alt", "Test").unwrap_err();
        assert!(format!("{err:#}").contains("Der Code ist ungültig"), "{err:#}");
    }
}
