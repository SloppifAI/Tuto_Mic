//! Mikrofon-Aufnahme in kurzen Segmenten (16 kHz mono WAV).
//!
//! Die Segmentierung macht "die letzten X Minuten löschen" einfach: Es werden ganze Segmente
//! vom Ende entfernt, bis mindestens X Minuten weg sind. Es wird also eher etwas mehr gelöscht
//! als zu wenig – im Zweifel zugunsten des Datenschutzes.

use crate::audio::{StreamResampler, WHISPER_RATE};
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

enum Msg {
    Samples(Vec<f32>),
    DeleteLast(Duration),
    Finish,
}

#[derive(Default)]
struct Shared {
    /// Pegel (RMS) des letzten Puffers als f32-Bits.
    level: AtomicU32,
    /// Gespeicherte Länge in 16-kHz-Frames (nach Löschungen).
    recorded_frames: AtomicU64,
    error: Mutex<Option<String>>,
}

pub struct Recorder {
    stream: cpal::Stream,
    tx: Sender<Msg>,
    writer: JoinHandle<Result<()>>,
    shared: Arc<Shared>,
    paused: Arc<AtomicBool>,
    pub device_name: String,
}

impl Recorder {
    pub fn start(audio_dir: &Path, segment_secs: u32) -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_input_device().context("Kein Mikrofon gefunden")?;
        let device_name = device
            .description()
            .map(|d| d.to_string())
            .unwrap_or_else(|_| "Standard-Mikrofon".into());
        let supported = device.default_input_config()?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;

        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let paused = Arc::new(AtomicBool::new(false));

        let writer = {
            let dir = audio_dir.to_path_buf();
            let shared = shared.clone();
            let segment_frames = (segment_secs.max(1) * WHISPER_RATE) as u64;
            std::thread::spawn(move || {
                let res = writer_loop(rx, dir, rate, segment_frames, &shared);
                if let Err(e) = &res {
                    *shared.error.lock().unwrap() = Some(format!("{e:#}"));
                }
                res
            })
        };

        let config: cpal::StreamConfig = supported.config();
        let ctx = CallbackCtx {
            channels,
            tx: tx.clone(),
            shared: shared.clone(),
            paused: paused.clone(),
        };
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config, ctx)?,
            SampleFormat::I16 => build::<i16>(&device, config, ctx)?,
            SampleFormat::U16 => build::<u16>(&device, config, ctx)?,
            SampleFormat::I32 => build::<i32>(&device, config, ctx)?,
            SampleFormat::I8 => build::<i8>(&device, config, ctx)?,
            SampleFormat::U8 => build::<u8>(&device, config, ctx)?,
            f => bail!("Sample-Format {f:?} wird nicht unterstützt"),
        };
        stream.play()?;
        Ok(Self {
            stream,
            tx,
            writer,
            shared,
            paused,
            device_name,
        })
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn delete_last(&self, d: Duration) {
        let _ = self.tx.send(Msg::DeleteLast(d));
    }

    pub fn recorded(&self) -> Duration {
        Duration::from_secs_f64(self.shared.recorded_frames.load(Ordering::Relaxed) as f64 / WHISPER_RATE as f64)
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().unwrap().clone()
    }

    /// Beendet die Aufnahme und schreibt das letzte Segment.
    pub fn stop(self) -> Result<()> {
        drop(self.stream);
        let _ = self.tx.send(Msg::Finish);
        self.writer
            .join()
            .map_err(|_| anyhow::anyhow!("Schreib-Thread abgestürzt"))?
    }
}

struct CallbackCtx {
    channels: usize,
    tx: Sender<Msg>,
    shared: Arc<Shared>,
    paused: Arc<AtomicBool>,
}

fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, ctx: CallbackCtx) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let CallbackCtx {
        channels,
        tx,
        shared,
        paused,
    } = ctx;
    let err_shared = shared.clone();
    let stream = device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if paused.load(Ordering::Relaxed) {
                shared.level.store(0f32.to_bits(), Ordering::Relaxed);
                return;
            }
            let mono: Vec<f32> = data
                .chunks(channels)
                .map(|frame| frame.iter().map(|&s| f32::from_sample_(s)).sum::<f32>() / channels as f32)
                .collect();
            let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len().max(1) as f32).sqrt();
            shared.level.store(rms.to_bits(), Ordering::Relaxed);
            let _ = tx.send(Msg::Samples(mono));
        },
        move |err| {
            log::error!("Audio-Stream: {err}");
            *err_shared.error.lock().unwrap() = Some(err.to_string());
        },
        None,
    )?;
    Ok(stream)
}

struct Segment {
    path: PathBuf,
    frames: u64,
}

struct OpenSegment {
    writer: hound::WavWriter<BufWriter<File>>,
    seg: Segment,
}

fn writer_loop(rx: Receiver<Msg>, dir: PathBuf, rate: u32, segment_frames: u64, shared: &Shared) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: WHISPER_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut resampler = StreamResampler::new(rate)?;
    let mut done: Vec<Segment> = Vec::new();
    let mut open: Option<OpenSegment> = None;
    let mut next_index = 1u32;
    let mut buf = Vec::new();

    let close = |open: &mut Option<OpenSegment>, done: &mut Vec<Segment>| -> Result<()> {
        if let Some(o) = open.take() {
            o.writer.finalize()?;
            done.push(o.seg);
        }
        Ok(())
    };

    for msg in rx {
        match msg {
            Msg::Samples(samples) => {
                buf.clear();
                resampler.push(&samples, &mut buf)?;
                for &s in &buf {
                    if open.is_none() {
                        let path = dir.join(format!("seg_{next_index:05}.wav"));
                        next_index += 1;
                        open = Some(OpenSegment {
                            writer: hound::WavWriter::create(&path, spec)?,
                            seg: Segment { path, frames: 0 },
                        });
                    }
                    let o = open.as_mut().unwrap();
                    o.writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
                    o.seg.frames += 1;
                    if o.seg.frames >= segment_frames {
                        close(&mut open, &mut done)?;
                    }
                }
            }
            Msg::DeleteLast(d) => {
                close(&mut open, &mut done)?;
                let mut remaining = (d.as_secs_f64() * WHISPER_RATE as f64) as u64;
                while remaining > 0 {
                    let Some(seg) = done.pop() else { break };
                    std::fs::remove_file(&seg.path)?;
                    remaining = remaining.saturating_sub(seg.frames);
                }
                log::info!("{} s Aufnahme gelöscht", d.as_secs());
            }
            Msg::Finish => break,
        }
        let total = done.iter().map(|s| s.frames).sum::<u64>() + open.as_ref().map_or(0, |o| o.seg.frames);
        shared.recorded_frames.store(total, Ordering::Relaxed);
    }
    close(&mut open, &mut done)
}
