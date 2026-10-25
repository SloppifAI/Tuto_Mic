//! Transkription mit whisper.cpp (über whisper-rs) und Aufbereitung als Text mit Zeitstempeln.

use crate::config::Config;
use anyhow::{Context, Result};
use regex::Regex;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

/// Setzt Stil (Groß-/Kleinschreibung, Satzzeichen) und Domäne.
const INITIAL_PROMPT: &str =
    "Mitschnitt einer Unterrichtsstunde an einer deutschen Schule. Lehrkraft und Schülerinnen und Schüler sprechen.";

/// Höchstlänge der Namensliste im Prompt. Whisper nutzt nur etwa 224 Token Prompt und schneidet vorne ab; so
/// bleibt der Stilsatz erhalten, auch wenn seltene Namen in viele Token zerfallen.
const MAX_PROMPT_NAMES_CHARS: usize = 300;

/// Prompt mit den Namen der Klassenliste: Whisper schreibt Namen, die im Prompt stehen, häufiger richtig, und
/// richtig geschriebene Namen erkennt die Pseudonymisierung sicher. Erst alle Vornamen, dann Nachnamen, solange
/// Platz ist; ohne Doppelungen und nur ganze Namen.
fn initial_prompt(class_list: &[String]) -> String {
    let people: Vec<Vec<&str>> = class_list.iter().map(|p| p.split_whitespace().collect()).collect();
    let first_names = people.iter().filter_map(|parts| parts.first());
    let surnames = people
        .iter()
        .filter(|parts| parts.len() > 1)
        .filter_map(|parts| parts.last());

    let mut names: Vec<&str> = Vec::new();
    let mut len = 0;
    for name in first_names.chain(surnames) {
        if names.iter().any(|n| n.to_lowercase() == name.to_lowercase()) {
            continue;
        }
        let added = name.chars().count() + if names.is_empty() { 0 } else { 2 };
        if len + added > MAX_PROMPT_NAMES_CHARS {
            break;
        }
        len += added;
        names.push(name);
    }
    if names.is_empty() {
        INITIAL_PROMPT.to_string()
    } else {
        format!("{INITIAL_PROMPT} Namen in der Klasse: {}.", names.join(", "))
    }
}

#[derive(Clone, Debug)]
pub struct Segment {
    pub start_ms: i64,
    pub text: String,
}

fn load_model(model: &Path) -> Result<WhisperContext> {
    // Vor der GPU-Initialisierung prüfen: Ohne Datei braucht es keine.
    anyhow::ensure!(
        model.is_file(),
        "Whisper-Modell nicht ladbar: {} fehlt",
        model.display()
    );
    let mut params = WhisperContextParameters::default();
    params.gpu_device(crate::gpu::select_gpu());
    WhisperContext::new_with_params(model, params)
        .with_context(|| format!("Whisper-Modell nicht ladbar: {}", model.display()))
}

/// Lädt das Modell und startet Whisper einmal, ohne zu transkribieren. Erst beim Start wählt whisper.cpp das
/// Gerät (GPU oder CPU, danach in [`crate::gpu::detected`]).
pub fn probe_device(model: &Path) -> Result<()> {
    load_model(model)?
        .create_state()
        .context("Whisper konnte nicht gestartet werden")?;
    Ok(())
}

/// Geladenes Whisper-Modell samt Arbeitszustand, mehrfach nutzbar: Die Mitschrift während der Aufnahme
/// (`live`) lädt das Modell einmal und transkribiert damit Abschnitt für Abschnitt.
pub struct Transcriber {
    /// Hält auch den Kontext (das Modell) am Leben.
    state: WhisperState,
    language: String,
    prompt: String,
    vad_model: Option<String>,
    threads: i32,
}

impl Transcriber {
    /// Lädt das Modell. `threads` begrenzt die CPU-Threads von whisper.cpp (`None`: bis zu 8).
    pub fn load(cfg: &Config, threads: Option<usize>) -> Result<Self> {
        let state = load_model(&cfg.whisper_model)?
            .create_state()
            .context("Whisper konnte nicht gestartet werden")?;
        let vad_model = if cfg.vad_model.is_file() {
            Some(cfg.vad_model.to_string_lossy().into_owned())
        } else {
            log::warn!(
                "VAD-Modell fehlt ({}) – Transkription ohne VAD",
                cfg.vad_model.display()
            );
            None
        };
        let threads = threads.unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get().min(8)));
        Ok(Self {
            state,
            language: cfg.language.clone(),
            prompt: initial_prompt(&cfg.class_list),
            vad_model,
            threads: threads.max(1) as i32,
        })
    }

    /// Transkribiert `samples` (16 kHz mono); Zeitstempel relativ zum Anfang von `samples`.
    pub fn run(
        &mut self,
        samples: &[f32],
        progress: Option<Box<dyn FnMut(i32)>>,
        abort: Arc<AtomicBool>,
    ) -> Result<Vec<Segment>> {
        let mut params = FullParams::new(SamplingStrategy::BeamSearch {
            beam_size: 5,
            patience: -1.0,
        });
        params.set_language(Some(&self.language));
        params.set_initial_prompt(&self.prompt);
        params.set_n_threads(self.threads);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        if let Some(vad) = &self.vad_model {
            params.set_vad_model_path(Some(vad));
            params.enable_vad(true);
        }
        // whisper-rs gibt die Closures nie frei. Die Mitschrift ruft oft auf und meldet deshalb keinen Fortschritt.
        if let Some(progress) = progress {
            params.set_progress_callback_safe(progress);
        }
        // whisper-rs 0.16 legt die Closure als `Box<dyn FnMut>` ab, liest sie im Callback aber als `F`. Nur wenn
        // `F` selbst `Box<dyn FnMut() -> bool>` ist, passt beides zusammen. Sonst liest der Callback Zufallswerte
        // und bricht sporadisch ab („failed to encode“; mit Vulkan fast immer).
        let abort_callback: Box<dyn FnMut() -> bool> = Box::new(move || abort.load(Ordering::Relaxed));
        params.set_abort_callback_safe(abort_callback);

        self.state
            .full(params, samples)
            .context("Transkription fehlgeschlagen oder abgebrochen")?;

        let mut out = Vec::new();
        for seg in self.state.as_iter() {
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
}

/// Lädt das Modell und transkribiert `samples` in einem Durchgang (Auswertung ohne Mitschrift, Importe).
pub fn transcribe(
    samples: &[f32],
    cfg: &Config,
    progress: impl FnMut(i32) + 'static,
    abort: Arc<AtomicBool>,
) -> Result<Vec<Segment>> {
    Transcriber::load(cfg, None)?.run(samples, Some(Box::new(progress)), abort)
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

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn prompt_without_class_list_is_unchanged() {
        assert_eq!(initial_prompt(&[]), INITIAL_PROMPT);
        assert_eq!(initial_prompt(&list(&["  "])), INITIAL_PROMPT);
    }

    #[test]
    fn prompt_lists_first_names_then_surnames_without_duplicates() {
        let prompt = initial_prompt(&list(&[
            "Lena Wagner",
            "Tom Becker",
            "lena Özdemir",
            "Niklas",
            "Jörg Wagner",
        ]));
        assert_eq!(
            prompt,
            format!("{INITIAL_PROMPT} Namen in der Klasse: Lena, Tom, Niklas, Jörg, Wagner, Becker, Özdemir.")
        );
    }

    #[test]
    fn prompt_is_capped_at_whole_names() {
        let class: Vec<String> = (0..200).map(|i| format!("Vorname{i} Nachname{i}")).collect();
        let prompt = initial_prompt(&class);
        let names = prompt
            .strip_prefix(&format!("{INITIAL_PROMPT} Namen in der Klasse: "))
            .and_then(|rest| rest.strip_suffix('.'))
            .expect("Prompt hat das erwartete Format");
        assert!(names.chars().count() <= MAX_PROMPT_NAMES_CHARS);
        // Nur ganze Namen; Vornamen haben Vorrang vor Nachnamen.
        assert!(names.starts_with("Vorname0, Vorname1, "));
        assert!(
            names
                .split(", ")
                .all(|n| n.strip_prefix("Vorname").is_some_and(|i| i.parse::<u32>().is_ok()))
        );
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
