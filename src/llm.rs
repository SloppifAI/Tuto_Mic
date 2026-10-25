//! Lokales LLM über die HTTP-API von Ollama.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::time::Instant;

pub struct Ollama {
    base: String,
}

pub struct Generation {
    pub text: String,
    pub prompt_tokens: u64,
    pub output_tokens: u64,
    pub secs: f64,
}

impl Ollama {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
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
        let tags: Tags = ureq::get(format!("{}/api/tags", self.base))
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
        let resp: Resp = ureq::post(format!("{}/api/generate", self.base))
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
