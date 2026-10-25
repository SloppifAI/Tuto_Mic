//! Mitschrift während der Aufnahme: Whisper transkribiert die Stunde schon, während sie läuft, sodass nach
//! „Beenden und auswerten“ nur noch der letzte Rest fehlt.
//!
//! Der Schreib-Thread des Recorders schickt jedes abgeschlossene Segment (dieselben Samples, die er verschlüsselt
//! speichert) über einen unbegrenzten Kanal an einen eigenen Thread ([`Sink`]). Der Recorder blockiert so nie,
//! auch wenn Whisper hinterherhinkt; die Segmente müssen nicht wieder von der Platte gelesen und entschlüsselt
//! werden. Der Thread sammelt das Audio, schneidet es an Sprechpausen in Abschnitte von etwa 30–60 s
//! ([`find_cut`]) und transkribiert sie mit einem einmal geladenen Modell.
//!
//! Positionen zählen in Frames der gespeicherten Aufnahme (16 kHz, ohne Pausen, nach Löschungen), genau wie die
//! Segmente auf der Platte hintereinander. Löscht die Lehrkraft die letzten Minuten, verwirft [`Timeline`] alle
//! Abschnitte dahinter; ein Abschnitt, der über die neue Endstelle reicht, wird ganz verworfen und sein
//! behaltener Teil neu transkribiert.
//!
//! Der Text bleibt im Arbeitsspeicher und wird nicht angezeigt (der Bildschirm im Klassenraum ist oft für alle
//! sichtbar), nur der Fortschritt. Nach der Aufnahme nutzt die Pipeline die Mitschrift statt einer vollständigen
//! Transkription; ist sie unbrauchbar, transkribiert sie wie bisher alles aus den Segmenten ([`decide`]).

use crate::audio::WHISPER_RATE;
use crate::config::Config;
use crate::transcribe::{Segment, Transcriber};
use anyhow::{Context, Result, anyhow};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const RATE: usize = WHISPER_RATE as usize;

/// Nachricht vom Recorder an die Mitschrift.
pub enum Event {
    /// Abgeschlossenes Segment ab Frame `start` der gespeicherten Aufnahme.
    Audio { start: u64, samples: Vec<f32> },
    /// Die Aufnahme wurde hinten gekürzt und ist jetzt `len` Frames lang.
    Truncate(u64),
}

/// Sendeseite für den Recorder. Senden blockiert nie (unbegrenzter Kanal); ist die Mitschrift schon beendet
/// (Fehler, Abbruch), gehen die Nachrichten ins Leere – die Aufnahme läuft davon unberührt weiter.
#[derive(Clone)]
pub struct Sink(Sender<Event>);

impl Sink {
    pub fn audio(&self, start: u64, samples: Vec<f32>) {
        let _ = self.0.send(Event::Audio { start, samples });
    }

    pub fn truncate(&self, len: u64) {
        let _ = self.0.send(Event::Truncate(len));
    }
}

/// Sink mit offenem Empfänger, für Tests des Recorders.
#[cfg(test)]
pub(crate) fn channel() -> (Sink, Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    (Sink(tx), rx)
}

/// Wo Abschnitte geschnitten werden, in Frames.
#[derive(Clone, Copy, Debug)]
pub struct ChunkParams {
    /// Kürzester Abschnitt; erst ab dieser Länge wird nach einer Pause gesucht.
    pub min: usize,
    /// Längster Abschnitt; ohne Pause wird hier spätestens geschnitten (an der leisesten Stelle).
    pub max: usize,
    /// Fensterlänge für die Pausensuche.
    pub window: usize,
    /// Abstand der geprüften Fenster.
    pub step: usize,
}

impl Default for ChunkParams {
    fn default() -> Self {
        Self {
            min: 30 * RATE,
            max: 60 * RATE,
            window: RATE * 3 / 10,
            step: RATE / 10,
        }
    }
}

/// Ein Fenster gilt als Pause, wenn es leiser ist als dieser Anteil des typischen (Median-)Pegels …
const PAUSE_FACTOR: f32 = 0.35;
/// … oder fast digital still ist (etwa −60 dBFS).
const PAUSE_FLOOR: f32 = 1e-3;

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

/// Sucht, wo der nächste Abschnitt von `samples` endet. Geschnitten wird in der Mitte der spätesten Pause
/// zwischen `min` und `max`, damit kein Wort zerteilt wird und der Abschnitt möglichst lang ist. Ohne Pause wartet
/// die Suche auf mehr Audio; ist `max` erreicht, schneidet sie an der leisesten Stelle. `None`: noch nicht
/// schneiden. Mit `finishing` (Aufnahme beendet) geht der Rest bis `max` am Stück.
pub fn find_cut(samples: &[f32], p: &ChunkParams, finishing: bool) -> Option<usize> {
    let n = samples.len();
    if n == 0 || (n < p.min && !finishing) {
        return None;
    }
    if finishing && n <= p.max {
        return Some(n);
    }
    let end = n.min(p.max);
    let half = p.window / 2;
    let step = p.step.max(1);
    // Typischer Pegel: Median über alle Fenster bis `end`. Relativ statt fest, weil Mikrofon und Raum den Pegel
    // stark verändern.
    let mut levels: Vec<f32> = samples[..end].chunks(p.window.max(1)).map(rms).collect();
    levels.sort_by(f32::total_cmp);
    let threshold = (levels[levels.len() / 2] * PAUSE_FACTOR).max(PAUSE_FLOOR);

    let lo = p.min.max(half);
    let hi = end.min(n - half);
    let mut quietest: Option<(f32, usize)> = None;
    let mut center = hi;
    while center >= lo && hi >= lo {
        let level = rms(&samples[center - half..center + half]);
        if level <= threshold {
            return Some(center);
        }
        if quietest.is_none_or(|(q, _)| level < q) {
            quietest = Some((level, center));
        }
        if center < lo + step {
            break;
        }
        center -= step;
    }
    (n >= p.max || finishing).then(|| quietest.map_or(end, |(_, c)| c))
}

/// Transkribierter Abschnitt der Aufnahme `[start, end)`.
struct Chunk {
    start: u64,
    end: u64,
    /// Zeitstempel ab Beginn der Aufnahme.
    segments: Vec<Segment>,
    /// Audio des Abschnitts, falls er wegen einer Löschung neu transkribiert werden muss. Nur für die letzten
    /// [`RETAIN_SECS`] behalten, ältere Abschnitte erreicht „Letzte Minuten verwerfen“ nicht.
    audio: Option<Vec<i16>>,
}

/// So weit reicht „Letzte Minuten verwerfen“ zurück (höchstens 30 min) plus ein Abschnitt.
const RETAIN_SECS: u64 = 31 * 60;

/// Buchführung der Mitschrift ohne Whisper: welches Audio wartet, welche Abschnitte fertig sind und was eine
/// Löschung davon ungültig macht.
pub struct Timeline {
    /// Noch nicht transkribiertes Audio ab Frame `pending_start`.
    pending: Vec<f32>,
    pending_start: u64,
    done: Vec<Chunk>,
    retain: u64,
    /// Grund, warum die Mitschrift nicht mehr zur Aufnahme passt (dann gilt die vollständige Transkription).
    broken: Option<String>,
}

impl Timeline {
    /// `retain`: so viele Frames vom Ende her behalten fertige Abschnitte ihr Audio.
    pub fn new(retain: u64) -> Self {
        Self {
            pending: Vec::new(),
            pending_start: 0,
            done: Vec::new(),
            retain,
            broken: None,
        }
    }

    /// Empfangene Länge der Aufnahme in Frames.
    pub fn received(&self) -> u64 {
        self.pending_start + self.pending.len() as u64
    }

    /// Bis hierhin ist die Aufnahme lückenlos transkribiert.
    pub fn transcribed(&self) -> u64 {
        self.done.last().map_or(0, |c| c.end)
    }

    pub fn broken(&self) -> Option<&str> {
        self.broken.as_deref()
    }

    fn mark_broken(&mut self, reason: String) {
        if self.broken.is_none() {
            log::warn!("Mitschrift: {reason}");
            self.broken = Some(reason);
        }
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Audio { start, samples } => self.push(start, &samples),
            Event::Truncate(len) => self.truncate(len),
        }
    }

    /// Hängt ein Segment an. Es muss genau am bisherigen Ende beginnen.
    pub fn push(&mut self, start: u64, samples: &[f32]) {
        if start != self.received() {
            self.mark_broken(format!("Segment bei Frame {start}, erwartet {}", self.received()));
            return;
        }
        self.pending.extend_from_slice(samples);
    }

    /// Kürzt auf `len` Frames: Abschnitte dahinter fallen weg, ein Abschnitt über die Schnittstelle wird ganz
    /// verworfen und sein behaltener Teil wartet erneut auf Transkription.
    pub fn truncate(&mut self, len: u64) {
        if len >= self.received() {
            return;
        }
        if len >= self.pending_start {
            self.pending.truncate((len - self.pending_start) as usize);
            return;
        }
        self.pending.clear();
        self.pending_start = len;
        while self.done.last().is_some_and(|c| c.end > len) {
            let chunk = self.done.pop().expect("eben geprüft");
            if chunk.start < len {
                let keep = (len - chunk.start) as usize;
                match chunk.audio {
                    Some(audio) => {
                        self.pending = audio[..keep]
                            .iter()
                            .map(|&s| f32::from(s) / f32::from(i16::MAX))
                            .collect();
                        self.pending_start = chunk.start;
                    }
                    None => self.mark_broken(format!(
                        "Löschung bis Frame {len} trifft einen Abschnitt ohne gespeichertes Audio"
                    )),
                }
            }
        }
    }

    /// Nimmt den nächsten Abschnitt zum Transkribieren heraus: `(start, audio)`.
    pub fn next_chunk(&mut self, p: &ChunkParams, finishing: bool) -> Option<(u64, Vec<f32>)> {
        let cut = find_cut(&self.pending, p, finishing)?;
        let rest = self.pending.split_off(cut);
        let audio = std::mem::replace(&mut self.pending, rest);
        let start = self.pending_start;
        self.pending_start += cut as u64;
        Some((start, audio))
    }

    /// Trägt den Text eines mit [`Self::next_chunk`] entnommenen Abschnitts ein (Zeitstempel relativ zum Abschnitt).
    pub fn complete(&mut self, start: u64, audio: &[f32], mut segments: Vec<Segment>) {
        debug_assert_eq!(start, self.transcribed(), "Abschnitte kommen der Reihe nach");
        let offset_ms = (start * 1000 / u64::from(WHISPER_RATE)) as i64;
        for s in &mut segments {
            s.start_ms += offset_ms;
        }
        self.done.push(Chunk {
            start,
            end: start + audio.len() as u64,
            segments,
            audio: Some(
                audio
                    .iter()
                    .map(|s| (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
                    .collect(),
            ),
        });
        let horizon = self.received().saturating_sub(self.retain);
        for c in self.done.iter_mut().take_while(|c| c.end <= horizon) {
            c.audio = None;
        }
    }

    /// Fertige Mitschrift; nur gültig, wenn alles transkribiert ist und nichts aus dem Tritt kam.
    pub fn finish(self) -> Result<LiveTranscript> {
        if let Some(reason) = self.broken {
            anyhow::bail!("Mitschrift passt nicht zur Aufnahme: {reason}");
        }
        anyhow::ensure!(self.pending.is_empty(), "Mitschrift unvollständig");
        Ok(LiveTranscript {
            frames: self.pending_start,
            chunks: self.done.len(),
            segments: self.done.into_iter().flat_map(|c| c.segments).collect(),
        })
    }
}

/// Ergebnis der Mitschrift.
#[derive(Debug, Default)]
pub struct LiveTranscript {
    /// Zeitstempel ab Beginn der Aufnahme.
    pub segments: Vec<Segment>,
    /// Länge der transkribierten Aufnahme in Frames.
    pub frames: u64,
    /// Zahl der transkribierten Abschnitte.
    pub chunks: usize,
}

/// Fortschritt für die Anzeige während der Aufnahme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub transcribed: Duration,
    /// Mitschrift gescheitert; nach der Aufnahme wird vollständig transkribiert.
    pub failed: bool,
}

#[derive(Default)]
struct Shared {
    transcribed: AtomicU64,
    received: AtomicU64,
    failed: AtomicBool,
}

/// Läuft die Mitschrift so weit hinterher, dass sie beim Beenden weniger als die Hälfte geschafft hat (bei
/// mindestens 5 min Aufnahme), transkribiert die Pipeline lieber alles am Stück: Das dauert kaum länger, und
/// Whisper hat über die ganze Stunde mehr Zusammenhang als über einzelne Abschnitte.
pub fn too_far_behind(transcribed: u64, received: u64) -> bool {
    received >= 5 * 60 * u64::from(WHISPER_RATE) && transcribed < received / 2
}

/// Ob die Pipeline die Mitschrift nutzt. `recorded_frames`: Länge der gespeicherten Segmente. `Err` mit dem
/// Grund, wenn sie stattdessen vollständig transkribieren soll.
pub fn decide(result: Result<LiveTranscript>, recorded_frames: Option<u64>) -> Result<LiveTranscript, String> {
    let live = result.map_err(|e| format!("{e:#}"))?;
    if live.frames == 0 {
        return Err("keine Aufnahme mitgeschrieben".into());
    }
    if let Some(recorded) = recorded_frames
        && recorded != live.frames
    {
        return Err(format!(
            "Länge passt nicht zur Aufnahme ({:.1} s statt {:.1} s)",
            live.frames as f64 / f64::from(WHISPER_RATE),
            recorded as f64 / f64::from(WHISPER_RATE)
        ));
    }
    Ok(live)
}

/// Laufende Mitschrift. Beim Freigeben ohne [`Self::join`] bricht sie ab (z. B. App beendet).
pub struct LiveTranscriber {
    handle: Mutex<Option<JoinHandle<Result<LiveTranscript>>>>,
    shared: Arc<Shared>,
    abort: Arc<AtomicBool>,
}

/// Nie den Text ausgeben, auch nicht versehentlich in einem Log.
impl std::fmt::Debug for LiveTranscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveTranscriber(…)")
    }
}

impl Drop for LiveTranscriber {
    fn drop(&mut self) {
        self.abort.store(true, Ordering::Relaxed);
    }
}

/// Startet die Mitschrift. Das Modell lädt sie erst, wenn der erste Abschnitt fertig ist; eine sehr kurze
/// Aufnahme kostet so nichts zusätzlich. `tuto`: Zugang, um die Klassenliste für den Whisper-Hinweis zu laden.
pub fn start(cfg: Config, tuto: Option<crate::tuto::Session>) -> (Sink, LiveTranscriber) {
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared::default());
    let abort = Arc::new(AtomicBool::new(false));
    let handle = {
        let (shared, abort) = (shared.clone(), abort.clone());
        std::thread::Builder::new().name("mitschrift".into()).spawn(move || {
            let res = worker(&rx, cfg, tuto, &shared, &abort);
            if let Err(e) = &res {
                shared.failed.store(true, Ordering::Relaxed);
                if !abort.load(Ordering::Relaxed) {
                    log::warn!("Mitschrift beendet: {e:#}");
                }
            }
            res
        })
    };
    let handle = match handle {
        Ok(h) => Some(h),
        Err(e) => {
            log::warn!("Mitschrift nicht gestartet: {e}");
            shared.failed.store(true, Ordering::Relaxed);
            None
        }
    };
    (
        Sink(tx),
        LiveTranscriber {
            handle: Mutex::new(handle),
            shared,
            abort,
        },
    )
}

impl LiveTranscriber {
    pub fn status(&self) -> Status {
        Status {
            transcribed: frames_to_duration(self.shared.transcribed.load(Ordering::Relaxed)),
            failed: self.shared.failed.load(Ordering::Relaxed),
        }
    }

    /// `(transkribiert, empfangen)` in Frames.
    pub fn backlog(&self) -> (u64, u64) {
        (
            self.shared.transcribed.load(Ordering::Relaxed),
            self.shared.received.load(Ordering::Relaxed),
        )
    }

    /// Fertig (Aufnahme beendet und Rest transkribiert, oder gescheitert).
    pub fn is_finished(&self) -> bool {
        self.handle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
    }

    /// Bricht eine laufende Transkription ab; [`Self::join`] kehrt dann bald zurück.
    pub fn abort(&self) {
        self.abort.store(true, Ordering::Relaxed);
    }

    /// Wartet auf das Ergebnis. Liefert es nur einmal.
    pub fn join(&self) -> Result<LiveTranscript> {
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .context("Mitschrift nicht verfügbar")?;
        handle.join().map_err(|_| anyhow!("Mitschrift abgestürzt"))?
    }

    /// Schon fertige Mitschrift (Tests der Pipeline ohne Whisper).
    #[cfg(test)]
    pub(crate) fn finished(result: Result<LiveTranscript>) -> Self {
        let handle = std::thread::spawn(move || result);
        Self {
            handle: Mutex::new(Some(handle)),
            shared: Arc::default(),
            abort: Arc::default(),
        }
    }
}

fn frames_to_duration(frames: u64) -> Duration {
    Duration::from_secs_f64(frames as f64 / f64::from(WHISPER_RATE))
}

/// Kürzeste Eingabe für whisper.cpp (etwas über einer Sekunde).
const SHORTEST: usize = RATE + RATE / 10;

/// CPU-Threads für Whisper während der Aufnahme: höchstens die Hälfte, damit Aufnahme und Oberfläche flüssig
/// bleiben (mit GPU rechnet ohnehin kaum die CPU).
fn live_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 4))
}

fn worker(
    rx: &Receiver<Event>,
    mut cfg: Config,
    tuto: Option<crate::tuto::Session>,
    shared: &Shared,
    abort: &Arc<AtomicBool>,
) -> Result<LiveTranscript> {
    let params = ChunkParams::default();
    let mut timeline = Timeline::new(RETAIN_SECS * u64::from(WHISPER_RATE));
    let mut tuto = tuto;
    let mut whisper: Option<Transcriber> = None;
    let mut input_open = true;
    loop {
        // Alles Angekommene übernehmen, ohne zu warten.
        while input_open {
            match rx.try_recv() {
                Ok(event) => timeline.apply(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => input_open = false,
            }
        }
        shared.received.store(timeline.received(), Ordering::Relaxed);
        shared.transcribed.store(timeline.transcribed(), Ordering::Relaxed);
        anyhow::ensure!(!abort.load(Ordering::Relaxed), "Mitschrift abgebrochen");
        if let Some(reason) = timeline.broken() {
            anyhow::bail!("{reason}");
        }

        if let Some((start, audio)) = timeline.next_chunk(&params, !input_open) {
            if whisper.is_none() {
                if let Some(session) = tuto.take() {
                    let line = crate::pipeline::add_tuto_roster(&mut cfg, session).unwrap_or_else(|e| e);
                    log::info!("Mitschrift: {line}");
                }
                whisper = Some(Transcriber::load(&cfg, Some(live_threads()))?);
            }
            let w = whisper.as_mut().expect("eben geladen");
            let t = Instant::now();
            // whisper.cpp verweigert Audio unter einer Sekunde; Stille am Ende ändert die Zeitstempel nicht.
            let segments = if audio.len() < SHORTEST {
                let mut padded = audio.clone();
                padded.resize(SHORTEST, 0.0);
                w.run(&padded, None, abort.clone())?
            } else {
                w.run(&audio, None, abort.clone())?
            };
            log::info!(
                "Mitschrift: {:.0}–{:.0} s in {:.1} s",
                start as f64 / f64::from(WHISPER_RATE),
                (start + audio.len() as u64) as f64 / f64::from(WHISPER_RATE),
                t.elapsed().as_secs_f64()
            );
            timeline.complete(start, &audio, segments);
            shared.transcribed.store(timeline.transcribed(), Ordering::Relaxed);
            continue;
        }
        if !input_open {
            break;
        }
        // Mit Zeitlimit, damit ein Abbruch auch ohne neue Segmente ankommt.
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => timeline.apply(event),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => input_open = false,
        }
    }
    timeline.finish()
}

/// Entwicklerwerkzeug (`tuto-mic simulate-live`): spielt eine Audiodatei in Segmenten wie der Recorder ein,
/// `speed`-fach schneller als Echtzeit (0: so schnell wie möglich), und misst, wie lange nach dem Ende noch
/// gerechnet wird. `delete`: bei `(nach, wie viel)` wie „Letzte Minuten verwerfen“ kürzen. Liefert die Mitschrift
/// und die Wartezeit nach dem Ende.
pub fn simulate(
    path: &std::path::Path,
    cfg: &Config,
    speed: f64,
    delete: Option<(Duration, Duration)>,
) -> Result<(LiveTranscript, Duration)> {
    let samples = crate::audio::load_file(path)?;
    let segment = cfg.segment_secs.max(1) as usize * RATE;
    let (sink, live) = start(cfg.clone(), None);
    let begin = Instant::now();
    let mut kept: u64 = 0;
    let mut deleted = false;
    for piece in samples.chunks(segment) {
        if speed > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(piece.len() as f64 / RATE as f64 / speed));
        }
        sink.audio(kept, piece.to_vec());
        kept += piece.len() as u64;
        // Wie „Letzte Minuten verwerfen“: ganze Segmente vom Ende, sobald `at` erreicht ist.
        if let Some((at, amount)) = delete
            && !deleted
            && frames_to_duration(kept) >= at
        {
            deleted = true;
            let remove = (amount.as_secs_f64() * RATE as f64) as u64;
            let len = kept.saturating_sub(remove.div_ceil(segment as u64) * segment as u64);
            eprintln!(
                "Lösche bei {:.0} s: Aufnahme jetzt {:.0} s",
                frames_to_duration(kept).as_secs_f64(),
                frames_to_duration(len).as_secs_f64()
            );
            kept = len;
            sink.truncate(len);
        }
        let (done, received) = live.backlog();
        eprintln!(
            "  {:6.1} s: empfangen {:5.0} s, mitgeschrieben {:5.0} s",
            begin.elapsed().as_secs_f64(),
            frames_to_duration(received).as_secs_f64(),
            frames_to_duration(done).as_secs_f64()
        );
    }
    drop(sink);
    let end = Instant::now();
    let result = live.join()?;
    anyhow::ensure!(result.frames == kept, "Länge {} statt {kept}", result.frames);
    Ok((result, end.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kleine Zahlen statt 16 kHz: 1 „Sekunde“ = 10 Frames.
    const P: ChunkParams = ChunkParams {
        min: 30,
        max: 60,
        window: 4,
        step: 1,
    };

    fn speech(len: usize) -> Vec<f32> {
        (0..len).map(|i| if i % 2 == 0 { 0.3 } else { -0.3 }).collect()
    }

    fn with_pause(len: usize, pause_at: usize, pause_len: usize) -> Vec<f32> {
        let mut s = speech(len);
        for v in &mut s[pause_at..pause_at + pause_len] {
            *v = 0.001;
        }
        s
    }

    #[test]
    fn waits_until_minimum_length() {
        assert_eq!(find_cut(&[], &P, false), None);
        assert_eq!(find_cut(&with_pause(29, 10, 6), &P, false), None);
    }

    #[test]
    fn cuts_in_the_latest_pause() {
        let mut s = with_pause(50, 33, 6);
        for v in &mut s[44..50] {
            *v = 0.0;
        }
        // In der späteren Pause (44..50, letztes ganzes Fenster 46..50), nicht in der ersten.
        assert_eq!(find_cut(&s, &P, false), Some(48));
        // Nur die erste Pause (33..39): letztes ganzes Fenster darin.
        assert_eq!(find_cut(&with_pause(50, 33, 6), &P, false), Some(37));
    }

    #[test]
    fn ignores_pauses_before_minimum() {
        assert_eq!(find_cut(&with_pause(50, 5, 10), &P, false), None);
    }

    #[test]
    fn without_pause_waits_then_cuts_at_quietest_place() {
        assert_eq!(find_cut(&speech(55), &P, false), None);
        let mut s = speech(80);
        // Etwas leiser, aber keine Pause: Bei `max` wird trotzdem dort geschnitten.
        for v in &mut s[40..44] {
            *v *= 0.6;
        }
        assert_eq!(find_cut(&s, &P, false), Some(42));
        // Gleichmäßig laut: irgendwo zwischen `min` und `max`, nie dahinter.
        let cut = find_cut(&speech(200), &P, false).unwrap();
        assert!((P.min..=P.max).contains(&cut), "{cut}");
    }

    #[test]
    fn pause_threshold_is_relative_to_level() {
        // Leises Mikrofon: Sprache bei 0.03, Pause bei 0.005 – absolut nicht still, relativ aber deutlich.
        let mut s: Vec<f32> = speech(50).iter().map(|v| v / 10.0).collect();
        for v in &mut s[38..42] {
            *v = 0.005;
        }
        assert_eq!(find_cut(&s, &P, false), Some(40));
    }

    #[test]
    fn finishing_takes_the_rest() {
        assert_eq!(find_cut(&speech(5), &P, true), Some(5));
        assert_eq!(find_cut(&speech(60), &P, true), Some(60));
        // Länger als `max`: auch am Ende in Abschnitten, an der leisesten Stelle.
        assert_eq!(find_cut(&with_pause(90, 40, 6), &P, true), Some(44));
    }

    #[test]
    fn default_params_are_whisper_sized() {
        let p = ChunkParams::default();
        assert_eq!((p.min, p.max), (30 * 16_000, 60 * 16_000));
        assert_eq!(p.window, 4_800);
        // Zwei Minuten Sprache mit Pause bei 40 s.
        let mut s = vec![0.2f32; 120 * 16_000];
        for (i, v) in s.iter_mut().enumerate() {
            if i % 2 == 1 {
                *v = -0.2;
            }
        }
        s[40 * 16_000..40 * 16_000 + 8_000].fill(0.0);
        let cut = find_cut(&s, &p, false).unwrap();
        assert!((40 * 16_000..40 * 16_000 + 8_000).contains(&cut), "{cut}");
    }

    fn seg(ms: i64, text: &str) -> Segment {
        Segment {
            start_ms: ms,
            text: text.into(),
        }
    }

    /// Transkribiert jeden fälligen Abschnitt mit einem Platzhalter-Text „start-end“.
    fn run_chunks(t: &mut Timeline, finishing: bool) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        while let Some((start, audio)) = t.next_chunk(&P, finishing) {
            let end = start + audio.len() as u64;
            t.complete(start, &audio, vec![seg(0, &format!("{start}-{end}"))]);
            out.push((start, end));
        }
        out
    }

    fn texts(t: Timeline) -> Vec<String> {
        t.finish().unwrap().segments.into_iter().map(|s| s.text).collect()
    }

    #[test]
    fn chunks_carry_remainder_and_absolute_times() {
        let mut t = Timeline::new(1000);
        t.push(0, &speech(20));
        assert_eq!(run_chunks(&mut t, false), []);
        // Pause bei 30..36 → Schnitt in ihrer Mitte (letztes ganzes Fenster 32..36), Rest wartet.
        t.push(20, &with_pause(20, 10, 6));
        assert_eq!(run_chunks(&mut t, false), [(0, 34)]);
        t.push(40, &speech(20));
        assert_eq!(run_chunks(&mut t, false), []);
        assert_eq!((t.transcribed(), t.received()), (34, 60));
        assert_eq!(run_chunks(&mut t, true), [(34, 60)]);
        let live = t.finish().unwrap();
        assert_eq!(live.frames, 60);
        assert_eq!(live.chunks, 2);
    }

    #[test]
    fn complete_shifts_timestamps_by_chunk_start() {
        let mut t = Timeline::new(u64::MAX);
        let ten_secs = vec![0.0; 10 * RATE];
        t.push(0, &ten_secs);
        t.push(10 * RATE as u64, &ten_secs);
        let (start, audio) = t.next_chunk(&ChunkParams::default(), true).unwrap();
        assert_eq!((start, audio.len()), (0, 20 * RATE));
        t.complete(0, &audio, vec![seg(1_500, "a")]);
        t.push(20 * RATE as u64, &ten_secs);
        let (start, audio) = t.next_chunk(&ChunkParams::default(), true).unwrap();
        t.complete(start, &audio, vec![seg(2_000, "b")]);
        let live = t.finish().unwrap();
        assert_eq!(
            live.segments.iter().map(|s| s.start_ms).collect::<Vec<_>>(),
            [1_500, 22_000]
        );
    }

    #[test]
    fn delete_within_pending_audio_keeps_transcribed_chunks() {
        let mut t = Timeline::new(1000);
        t.push(0, &with_pause(40, 30, 6));
        assert_eq!(run_chunks(&mut t, false), [(0, 34)]);
        t.push(40, &speech(20));
        t.truncate(50);
        assert_eq!((t.transcribed(), t.received()), (34, 50));
        run_chunks(&mut t, true);
        assert_eq!(texts(t), ["0-34", "34-50"]);
    }

    /// Drei fertige Abschnitte (0..34, 34..74, 74..114) und 6 Frames, die noch warten.
    fn three_chunks(retain: u64) -> Timeline {
        let mut t = Timeline::new(retain);
        for i in 0..3u64 {
            t.push(i * 40, &with_pause(40, 30, 6));
            run_chunks(&mut t, false);
        }
        let cuts: Vec<_> = t.done.iter().map(|c| (c.start, c.end)).collect();
        assert_eq!(cuts, [(0, 34), (34, 74), (74, 114)]);
        assert_eq!(t.received(), 120);
        t
    }

    #[test]
    fn delete_drops_later_chunks_and_requeues_straddling_part() {
        let mut t = three_chunks(1000);
        // Mitten in den zweiten Abschnitt löschen: Texte des zweiten und dritten weg, Anfang des zweiten wartet.
        t.truncate(44);
        assert_eq!((t.transcribed(), t.received()), (34, 44));
        assert_eq!(t.pending.len(), 10);

        // Danach geht die Aufnahme bei 44 weiter.
        t.push(44, &speech(40));
        run_chunks(&mut t, true);
        assert_eq!(texts(t), ["0-34", "34-84"]);
    }

    #[test]
    fn requeued_audio_is_the_original_audio() {
        let mut t = Timeline::new(1000);
        let audio: Vec<f32> = (0..40).map(|i| (i as f32 - 20.0) / 32_767.0 * 100.0).collect();
        t.push(0, &audio);
        let (start, chunk) = t.next_chunk(&P, true).unwrap();
        t.complete(start, &chunk, Vec::new());
        t.truncate(25);
        let restored: Vec<f32> = t.pending.clone();
        assert_eq!(restored.len(), 25);
        for (a, b) in restored.iter().zip(&audio) {
            assert!((a - b).abs() < 1e-4, "{a} {b}");
        }
    }

    #[test]
    fn delete_on_chunk_boundary_and_everything() {
        let mut t = three_chunks(1000);
        t.truncate(74);
        assert_eq!((t.transcribed(), t.received()), (74, 74));
        assert_eq!(t.pending.len(), 0);
        t.truncate(0);
        assert_eq!((t.transcribed(), t.received()), (0, 0));
        t.push(0, &speech(5));
        run_chunks(&mut t, true);
        assert_eq!(texts(t), ["0-5"]);
    }

    #[test]
    fn delete_beyond_retained_audio_breaks_sync() {
        let mut t = three_chunks(40);
        assert!(t.done[0].audio.is_none(), "Audio älterer Abschnitte verworfen");
        assert!(t.done[2].audio.is_some());
        // Im dritten Abschnitt geht es noch …
        t.truncate(100);
        assert!(t.broken().is_none());
        // … im ersten nicht mehr: Dann gilt die vollständige Transkription.
        t.truncate(10);
        assert!(t.broken().is_some());
        assert!(t.finish().is_err());
    }

    #[test]
    fn gap_in_audio_breaks_sync() {
        let mut t = Timeline::new(1000);
        t.push(0, &speech(10));
        t.push(20, &speech(10));
        assert!(t.broken().unwrap().contains("erwartet 10"));
    }

    #[test]
    fn delete_zero_or_more_than_received_is_harmless() {
        let mut t = Timeline::new(1000);
        t.push(0, &speech(10));
        t.truncate(10);
        t.truncate(500);
        assert_eq!(t.received(), 10);
    }

    fn transcript(frames: u64) -> LiveTranscript {
        LiveTranscript {
            frames,
            segments: vec![seg(0, "Guten Morgen.")],
            ..LiveTranscript::default()
        }
    }

    #[test]
    fn decide_uses_live_only_when_it_matches_the_recording() {
        assert!(decide(Ok(transcript(160_000)), Some(160_000)).is_ok());
        // Platte nicht lesbar: Die Mitschrift ist dann die einzige Quelle.
        assert!(decide(Ok(transcript(160_000)), None).is_ok());
        let err = decide(Ok(transcript(160_000)), Some(176_000)).unwrap_err();
        assert!(err.contains("10.0 s statt 11.0 s"), "{err}");
        let err = decide(Err(anyhow!("Whisper-Modell nicht ladbar")), Some(160_000)).unwrap_err();
        assert!(err.contains("nicht ladbar"), "{err}");
        assert!(decide(Ok(transcript(0)), Some(0)).is_err());
    }

    #[test]
    fn behind_only_counts_for_longer_recordings() {
        let min = 60 * 16_000;
        assert!(!too_far_behind(0, 4 * min));
        assert!(too_far_behind(0, 5 * min));
        assert!(too_far_behind(20 * min, 45 * min));
        assert!(!too_far_behind(25 * min, 45 * min));
    }

    #[test]
    fn sink_never_blocks_and_survives_a_finished_worker() {
        let (tx, rx) = mpsc::channel();
        let sink = Sink(tx);
        for i in 0..1000 {
            sink.audio(i * 160_000, vec![0.0; 16]);
        }
        drop(rx);
        sink.audio(0, vec![0.0; 16]);
        sink.truncate(0);
    }
}
