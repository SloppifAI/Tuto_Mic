//! Transkription mit whisper.cpp (über whisper-rs) und Aufbereitung als Text mit Zeitstempeln.

use crate::config::Config;
use anyhow::{Context, Result};
use regex::Regex;
use std::fmt::Write as _;
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
    let mut state = ctx.create_state().context("Whisper konnte nicht gestartet werden")?;

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

    state
        .full(params, samples)
        .context("Transkription fehlgeschlagen oder abgebrochen")?;

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
        Regex::new(r"(?i)Untertitel(ung|e)? (des|im Auftrag des) (ZDF|WDR|NDR|SWR|BR)|Amara\.org")
            .expect("festes, gültiges Muster")
    });
    RE.is_match(text)
}

/// Kompaktes Format für LLMs: `[mm:ss] text` pro Zeile.
pub fn to_text(segments: &[Segment]) -> String {
    segments.iter().fold(String::new(), |mut out, s| {
        let secs = s.start_ms / 1000;
        let _ = writeln!(out, "[{:02}:{:02}] {}", secs / 60, secs % 60, s.text);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_subtitle_hallucinations() {
        assert!(is_hallucination("Untertitelung des ZDF, 2020"));
        assert!(is_hallucination("Untertitel im Auftrag des ZDF für funk, 2017"));
        assert!(is_hallucination("Untertitel von Amara.org"));
        assert!(!is_hallucination("Wir schreiben heute die Untertitel für unser Video."));
    }

    #[test]
    fn formats_lines_with_timestamps() {
        let segments = [
            Segment {
                start_ms: 5_400,
                text: "Guten Morgen.".into(),
            },
            Segment {
                start_ms: 3_725_000,
                text: "Hausaufgabe.".into(),
            },
        ];
        assert_eq!(to_text(&segments), "[00:05] Guten Morgen.\n[62:05] Hausaufgabe.\n");
    }
}
