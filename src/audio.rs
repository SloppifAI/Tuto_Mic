//! Audio laden und auf 16 kHz mono bringen (das Format, das Whisper erwartet).

use anyhow::{Context, Result};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use std::path::Path;

pub const WHISPER_RATE: u32 = 16_000;

/// Streaming-Resampler auf 16 kHz mono. Hält seinen Zustand zwischen Aufrufen,
/// damit an Blockgrenzen keine Artefakte entstehen.
pub struct StreamResampler {
    inner: Option<Fft<f32>>,
    pending: Vec<f32>,
    out_buf: Vec<f32>,
    ratio: f64,
}

impl StreamResampler {
    pub fn new(from_rate: u32) -> Result<Self> {
        let inner = if from_rate == WHISPER_RATE {
            None
        } else {
            Some(Fft::new(
                from_rate as usize,
                WHISPER_RATE as usize,
                1024,
                1,
                FixedSync::Input,
            )?)
        };
        Ok(Self {
            inner,
            pending: Vec::new(),
            out_buf: Vec::new(),
            ratio: f64::from(WHISPER_RATE) / f64::from(from_rate),
        })
    }

    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        let Some(r) = self.inner.as_mut() else {
            out.extend_from_slice(input);
            return Ok(());
        };
        self.pending.extend_from_slice(input);
        loop {
            let need = r.input_frames_next();
            if self.pending.len() < need {
                return Ok(());
            }
            let max_out = r.output_frames_max();
            self.out_buf.resize(max_out, 0.0);
            let inp = InterleavedSlice::new(&self.pending[..need], 1, need)?;
            let mut outp = InterleavedSlice::new_mut(&mut self.out_buf, 1, max_out)?;
            let (used, written) = r.process_into_buffer(&inp, &mut outp, None)?;
            out.extend_from_slice(&self.out_buf[..written]);
            self.pending.drain(..used);
        }
    }

    /// Restliche Samples verarbeiten (mit Stille aufgefüllt, Überhang abgeschnitten).
    pub fn finish(mut self, out: &mut Vec<f32>) -> Result<()> {
        let Some(need) = self.inner.as_ref().map(rubato::Resampler::input_frames_next) else {
            return Ok(());
        };
        if self.pending.is_empty() {
            return Ok(());
        }
        let rest = (self.pending.len() as f64 * self.ratio).ceil() as usize;
        let pad = need - self.pending.len() % need;
        let tail = vec![0.0; pad];
        let before = out.len();
        self.push(&tail, out)?;
        out.truncate((before + rest).min(out.len()));
        Ok(())
    }
}

/// Liest die Aufnahme-Segmente einer Session (16 kHz mono WAV) in Dateinamen-Reihenfolge.
pub fn load_segments(audio_dir: &Path) -> Result<Vec<f32>> {
    let mut files: Vec<_> = std::fs::read_dir(audio_dir)
        .with_context(|| format!("Audio-Verzeichnis fehlt: {}", audio_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    files.sort();
    let mut samples = Vec::new();
    for f in files {
        let mut reader = hound::WavReader::open(&f)?;
        samples.extend(
            reader
                .samples::<i16>()
                .map(|s| s.map(|s| f32::from(s) / f32::from(i16::MAX)))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    Ok(samples)
}

/// Dekodiert eine beliebige Audiodatei (mp3, wav, flac, ogg, m4a, …) zu 16 kHz mono.
pub fn load_file(path: &Path) -> Result<Vec<f32>> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
    use symphonia::core::meta::MetadataOptions;

    let file = std::fs::File::open(path).with_context(|| format!("Datei nicht lesbar: {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format =
        symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())?;
    let track = format
        .default_track(TrackType::Audio)
        .context("keine Audiospur gefunden")?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .context("keine Audio-Codec-Parameter")?;
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default())?;

    let mut resampler: Option<StreamResampler> = None;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut mono: Vec<f32> = Vec::new();
    let mut out = Vec::new();
    while let Some(packet) = format.next_packet()? {
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let channels = buf.spec().channels().count().max(1);
        if resampler.is_none() {
            resampler = Some(StreamResampler::new(buf.spec().rate())?);
        }
        interleaved.resize(buf.samples_interleaved(), 0.0);
        buf.copy_to_slice_interleaved(&mut interleaved);
        mono.clear();
        mono.extend(
            interleaved
                .chunks(channels)
                .map(|f| f.iter().sum::<f32>() / channels as f32),
        );
        resampler.as_mut().expect("oben angelegt").push(&mono, &mut out)?;
    }
    if let Some(r) = resampler {
        r.finish(&mut out)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_duration() {
        let input: Vec<f32> = (0..48_000 * 3).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut r = StreamResampler::new(48_000).unwrap();
        let mut out = Vec::new();
        for chunk in input.chunks(777) {
            r.push(chunk, &mut out).unwrap();
        }
        r.finish(&mut out).unwrap();
        let expected = 16_000 * 3;
        assert!((out.len() as i64 - expected).abs() < 1100, "len {}", out.len());
    }

    #[test]
    fn loads_segments_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: WHISPER_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        // Absichtlich in umgekehrter Reihenfolge anlegen.
        for (name, value) in [("seg_00002.wav", 2000i16), ("seg_00001.wav", 1000)] {
            let mut w = hound::WavWriter::create(dir.path().join(name), spec).unwrap();
            for _ in 0..100 {
                w.write_sample(value).unwrap();
            }
            w.finalize().unwrap();
        }
        let samples = load_segments(dir.path()).unwrap();
        assert_eq!(samples.len(), 200);
        assert!(samples[0] < samples[199], "Segment 1 muss vor Segment 2 stehen");
    }
}
