//! Download der Whisper- und VAD-Modelle.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::Path;

pub struct ModelSpec {
    pub label: &'static str,
    pub file: &'static str,
    pub url: &'static str,
}

/// large-v3-turbo in voller Genauigkeit (~1,6 GB). Die 5-Bit-Variante (550 MB) erkennt etwas schlechter; Tuto Mic
/// setzt die Mindestqualität, die es später nur besser geben soll.
pub const WHISPER: ModelSpec = ModelSpec {
    label: "Whisper large-v3-turbo",
    file: "ggml-large-v3-turbo.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
};

/// Silero-VAD: verhindert Halluzinationen in Sprechpausen.
pub const VAD: ModelSpec = ModelSpec {
    label: "Silero VAD v5.1.2",
    file: "ggml-silero-v5.1.2.bin",
    url: "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin",
};

/// Lädt `url` nach `dest` (über eine `.part`-Datei, damit abgebrochene Downloads nicht als fertig gelten).
pub fn download(url: &str, dest: &Path, mut progress: impl FnMut(u64, Option<u64>)) -> Result<()> {
    std::fs::create_dir_all(dest.parent().context("Zielpfad ohne Verzeichnis")?)?;
    let mut resp = ureq::get(url)
        .call()
        .with_context(|| format!("Download fehlgeschlagen: {url}"))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let part = dest.with_extension("part");
    let mut file =
        std::fs::File::create(&part).with_context(|| format!("Datei nicht schreibbar: {}", part.display()))?;
    let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    if let Some(total) = total.filter(|&t| t != done) {
        bail!("Download unvollständig: {done} von {total} Bytes");
    }
    std::fs::rename(&part, dest).with_context(|| format!("Datei nicht verschiebbar: {}", dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_http::serve_once;

    #[test]
    fn downloads_to_destination_and_reports_progress() {
        let (url, _server) = serve_once(200, "modell-daten");
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("sub/model.bin");
        let mut last = (0, None);
        download(&format!("{url}/model.bin"), &dest, |done, total| last = (done, total)).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "modell-daten");
        assert!(!dest.with_extension("part").exists());
        assert_eq!(last, (12, Some(12)));
    }

    #[test]
    fn failed_download_leaves_no_model_file() {
        let (url, _server) = serve_once(404, "nicht gefunden");
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.bin");
        assert!(download(&url, &dest, |_, _| {}).is_err());
        assert!(!dest.exists());
    }
}
