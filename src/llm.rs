//! Lokales LLM über die HTTP-API von Ollama.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

pub struct Ollama {
    base: String,
    agent: ureq::Agent,
}

pub struct Generation {
    pub text: String,
    pub prompt_tokens: u64,
    pub output_tokens: u64,
    pub secs: f64,
}

impl Ollama {
    pub fn new(base: &str) -> Self {
        // Kurzer Verbindungs-Timeout: Eine falsche Adresse soll schnell auffallen. Die Generierung selbst darf
        // bei langen Stunden Minuten dauern und bekommt daher kein Gesamtlimit.
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(3)))
            .build()
            .new_agent();
        Self {
            base: base.trim_end_matches('/').to_string(),
            agent,
        }
    }

    /// Installierte Modelle; schlägt fehl, wenn Ollama nicht läuft.
    pub fn models(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Tags {
            models: Vec<Model>,
        }
        #[derive(Deserialize)]
        struct Model {
            name: String,
        }
        let tags: Tags = self
            .agent
            .get(format!("{}/api/tags", self.base))
            .config()
            .timeout_global(Some(Duration::from_secs(5)))
            .build()
            .call()
            .with_context(|| format!("Ollama nicht erreichbar unter {} – läuft `ollama serve`?", self.base))?
            .body_mut()
            .read_json()?;
        Ok(tags.models.into_iter().map(|m| m.name).collect())
    }

    pub fn generate(&self, model: &str, prompt: &str, num_ctx: u32) -> Result<Generation> {
        #[derive(Deserialize)]
        struct Resp {
            response: String,
            #[serde(default)]
            prompt_eval_count: u64,
            #[serde(default)]
            eval_count: u64,
        }
        let start = Instant::now();
        let body = json!({
            "model": model,
            "prompt": prompt,
            "stream": false,
            "think": false,
            "options": { "num_ctx": num_ctx, "temperature": 0.3 },
        });
        let resp: Resp = self
            .agent
            .post(format!("{}/api/generate", self.base))
            .send_json(&body)
            .with_context(|| format!("Ollama-Anfrage an Modell {model} fehlgeschlagen"))?
            .body_mut()
            .with_config()
            .limit(64 << 20)
            .read_json()?;
        Ok(Generation {
            text: strip_think(&resp.response),
            prompt_tokens: resp.prompt_eval_count,
            output_tokens: resp.eval_count,
            secs: start.elapsed().as_secs_f64(),
        })
    }
}

/// Manche Modelle liefern trotz `think: false` einen `<think>`-Block mit.
fn strip_think(s: &str) -> String {
    match (s.find("<think>"), s.find("</think>")) {
        (Some(a), Some(b)) if a < b => format!("{}{}", &s[..a], &s[b + "</think>".len()..]).trim().to_string(),
        _ => s.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_http::serve_once;

    #[test]
    fn strips_think_block() {
        assert_eq!(strip_think("<think>\nGrübel\n</think>\n\n# Titel"), "# Titel");
        assert_eq!(strip_think("  # Titel\n"), "# Titel");
        assert_eq!(strip_think("</think> kaputt <think>"), "</think> kaputt <think>");
    }

    #[test]
    fn generate_sends_request_and_parses_answer() {
        let (url, server) = serve_once(
            200,
            r#"{"response":"<think>x</think># Mathe","prompt_eval_count":12,"eval_count":3}"#,
        );
        let g = Ollama::new(&format!("{url}/"))
            .generate("qwen3.5:9b", "Fasse zusammen", 4096)
            .unwrap();
        assert_eq!(g.text, "# Mathe");
        assert_eq!((g.prompt_tokens, g.output_tokens), (12, 3));

        let req = server.join().unwrap();
        assert!(req.head.starts_with("POST /api/generate "), "{}", req.head);
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["model"], "qwen3.5:9b");
        assert_eq!(body["prompt"], "Fasse zusammen");
        assert_eq!(body["stream"], false);
        assert_eq!(body["think"], false);
        assert_eq!(body["options"]["num_ctx"], 4096);
    }

    #[test]
    fn lists_models() {
        let (url, _server) = serve_once(200, r#"{"models":[{"name":"qwen3.5:9b"},{"name":"nimble:9b"}]}"#);
        assert_eq!(Ollama::new(&url).models().unwrap(), ["qwen3.5:9b", "nimble:9b"]);
    }

    #[test]
    fn unreachable_server_gives_helpful_error() {
        // Port 9 (discard) ist lokal praktisch nie belegt.
        let err = Ollama::new("http://127.0.0.1:9").models().unwrap_err();
        assert!(format!("{err:#}").contains("ollama serve"), "{err:#}");
    }

    #[test]
    fn http_error_is_reported() {
        let (url, _server) = serve_once(404, r#"{"error":"model not found"}"#);
        assert!(Ollama::new(&url).generate("fehlt", "x", 1024).is_err());
    }
}
