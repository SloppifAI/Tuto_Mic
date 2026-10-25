use crate::config::Config;
use anyhow::{Context, Result};
use regex::Regex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Setzt Stil (Groß-/Kleinschreibung, Satzzeichen) und Domäne.
const INITIAL_PROMPT: &str =
    "Mitschnitt einer Unterrichtsstunde an einer deutschen Schule. Lehrkraft und Schülerinnen und Schüler sprechen.";

#[derive(Clone, Debug)]
pub struct Segment {
    pub start_ms: i64,
    pub text: String,
}

pub fn transcribe(
    samples: &[f32],
    cfg: &Config,
    progress: impl FnMut(i32) + 'static,
    abort: Arc<AtomicBool>,
) -> Result<Vec<Segment>> {
    let model = &cfg.whisper_model;
    let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default())
        .with_context(|| format!("Whisper-Modell nicht ladbar: {}", model.display()))?;
    let mut state = ctx.create_state()?;

    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });
    params.set_language(Some(&cfg.language));
    params.set_initial_prompt(INITIAL_PROMPT);
    params.set_n_threads(std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    let vad = cfg.vad_model.to_string_lossy().into_owned();
    if cfg.vad_model.is_file() {
        params.set_vad_model_path(Some(&vad));
        params.enable_vad(true);
    } else {
        log::warn!("VAD-Modell fehlt ({vad}) – Transkription ohne VAD");
    }
    params.set_progress_callback_safe(progress);
    params.set_abort_callback_safe(move || abort.load(Ordering::Relaxed));

    state.full(params, samples)?;

    let mut out = Vec::new();
    for seg in state.as_iter() {
        let text = seg.to_str_lossy()?.trim().to_string();
        if text.is_empty() || is_hallucination(&text) {
            continue;
        }
        out.push(Segment {
            start_ms: seg.start_timestamp() * 10,
            text,
        });
    }
    Ok(out)
}

/// Bekannte Whisper-Halluzinationen bei Stille/Musik (aus Untertitel-Trainingsdaten).
fn is_hallucination(text: &str) -> bool {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)Untertitel(ung|e) (des|im Auftrag des) (ZDF|WDR|NDR|SWR|BR)|Amara\.org").unwrap()
    });
    RE.is_match(text)
}

/// Kompaktes Format für LLMs: `[mm:ss] text` pro Zeile.
pub fn to_text(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|s| {
            let secs = s.start_ms / 1000;
            format!("[{:02}:{:02}] {}\n", secs / 60, secs % 60, s.text)
        })
        .collect()
}
