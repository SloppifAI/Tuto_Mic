# Tuto Mic

Unterrichtsstunden aufnehmen, **lokal** transkribieren, pseudonymisieren und zusammenfassen –
als Grundlage für Hausaufgaben im Tuto-Portal (Fachaufgaben + Fragen, die nur beantworten kann,
wer im Unterricht aufgepasst hat).

```
Mikrofon ──► Segmente (10 s, 16 kHz) ──► Whisper (lokal) ──► Pseudonymisierung ──► Zusammenfassung (Ollama, lokal)
   ▲  Start / Pause / Stop                                     Klassenliste → [Schüler A]          │
   └─ "letzte X min löschen"                                   E-Mail, Telefon, Herr/Frau X        ▼
                                                                                    export.json (ohne Audio/Namen)
                                                                                    → Aufgabenerstellung, z. B. Cloud
```

## Funktionen

- **Aufnahme** mit Start, Pause, Stop und großer Statusanzeige (später: Hardware-Taster + LED, siehe `src/control.rs`)
- **Rückwirkend löschen**: die letzten 1–30 Minuten der laufenden Aufnahme verwerfen
- **Transkription** mit whisper.cpp (large-v3-turbo) + Silero-VAD, komplett offline
- **Pseudonymisierung** vor jedem KI-Schritt (Klassenliste, Kontaktdaten, Anrede von Lehrkräften)
- **Zusammenfassung** über ein lokales Ollama-Modell
- **Audio wird nach der Transkription gelöscht** (abschaltbar)
- **Export-Paket** `export.json`: pseudonymisiertes Transkript + Zusammenfassung für die spätere Aufgabenerstellung
- **Dev-Modus** (`--dev` oder `TUTO_MIC_DEV=1`): Prompt-Editor, Audiodatei oder Text-Transkript importieren,
  Original-Transkript, Log

## Nutzung

```sh
tuto-mic                 # GUI
tuto-mic --dev           # GUI mit Dev-Modus
tuto-mic download        # Whisper- und VAD-Modell laden (~550 MB, alternativ per Button in der GUI)
tuto-mic process stunde.mp3 [--no-llm]   # ohne GUI, z. B. zum Prompt-Testen
tuto-mic process stunde.txt              # fertiges Transkript: ohne Whisper direkt pseudonymisieren und zusammenfassen
tuto-mic paths           # Pfade von Konfiguration, Prompts, Sessions, Modellen
```

`process` entscheidet nach der Endung: `.txt` und `.md` gelten als Text-Transkript (UTF-8, z. B. Demo-Daten),
alles andere als Audiodatei. Für Transkripte braucht es kein Whisper-Modell; in `export.json` sind dann
`audio_seconds` und `models.transcription` `null`.

Voraussetzung für die Zusammenfassung: [Ollama](https://ollama.com) läuft lokal und hat ein Modell,
z. B. `ollama pull qwen3.5:9b`. Ohne Ollama werden Transkript und pseudonymisiertes Transkript trotzdem erstellt.

### Prompt anpassen

Der Zusammenfassungs-Prompt `prompts/summary.md` ist in die App eingebaut. Zum Testen lässt er sich überschreiben:

- im Dev-Modus über den Tab **Prompts** (speichert nach `<config>/prompts/`), oder
- per Verzeichnis: `TUTO_MIC_PROMPTS=./meine-prompts tuto-mic process stunde.mp3`
  (Datei `summary.md`; fehlt sie, gilt die eingebaute Version).

Das Transkript (`[mm:ss] text` pro Zeile; importierte Text-Transkripte unverändert) ersetzt den Platzhalter
`{{TRANSKRIPT}}`; fehlt er, wird es angehängt.

## Entwicklung

Mit Nix (empfohlen, enthält Rust, rust-analyzer, cmake, clang, die GUI-/Audio-Bibliotheken und die Prüfwerkzeuge).
Mit direnv lädt `.envrc` die Umgebung automatisch, auch für den Editor (`direnv allow`).

```sh
nix develop                  # CPU
nix develop .#cuda           # NVIDIA, dann: cargo run --release --features cuda
cargo run -- --dev
```

Qualitätsprüfungen (laufen auch in der CI):

```sh
cargo test                                  # Unit-Tests, ohne Modelle, Mikrofon oder Ollama
cargo clippy --all-targets -- -D warnings   # Lints, inkl. clippy::pedantic (Cargo.toml)
cargo fmt --check
cargo deny check                            # Sicherheitslücken, Lizenzen (deny.toml)
cargo machete                               # ungenutzte Abhängigkeiten
```

Regeln für Beiträge (auch für KI-Agenten): `AGENTS.md`.

Ohne Nix: Rust (stable), cmake, clang/libclang; unter Linux zusätzlich
`libasound2-dev libxkbcommon-dev libwayland-dev`.

GPU-Features: `cuda` (NVIDIA), `vulkan` (AMD/Intel/NVIDIA), `metal` (macOS). Ohne Feature läuft Whisper
auf der CPU – funktioniert, ist für 45 min Unterricht aber deutlich langsamer.

### Builds

GitHub Actions:

- `ci.yml`: bei jedem Push und Pull Request Formatierung, Lints, Tests und Abhängigkeitsprüfung (nur Linux).
- `release.yml`: Builds für Linux, Windows und macOS (Apple Silicon, Metal). Ein Tag wie `v0.1.0` erzeugt ein
  GitHub-Release mit den Paketen zum Download; von Hand gestartet („Run workflow“) nur Test-Artefakte.

```sh
git tag v0.1.0 && git push origin v0.1.0   # Release erstellen
```

Noch offen: GPU-Builds für Linux/Windows (CUDA/Vulkan), Signierung (macOS-Notarisierung, Windows-Code-Signing),
Installer.

### Aufbau

| Modul | Aufgabe |
|---|---|
| `src/main.rs`, `src/lib.rs` | Einstieg (CLI/GUI) bzw. Bibliothek mit der gesamten Logik |
| `src/app.rs` | egui-Oberfläche |
| `src/control.rs` | Befehle (Start/Pause/Stop/Löschen) und Aufnahmezustand – Schnittstelle für späteren Hardware-Taster |
| `src/recorder.rs` | Mikrofon (cpal), Resampling, Segment-Dateien, rückwirkendes Löschen |
| `src/audio.rs` | Audio laden/dekodieren (symphonia), Resampling auf 16 kHz (rubato) |
| `src/transcribe.rs` | whisper.cpp über whisper-rs |
| `src/anonymize.rs` | Pseudonymisierung |
| `src/llm.rs` | Ollama-HTTP-API |
| `src/pipeline.rs` | Ablauf nach der Aufnahme, Export |
| `src/prompts.rs`, `prompts/` | eingebauter + überschreibbarer Prompt |
| `src/icons.rs` | selbst gezeichnete Icons (egui-Schriften enthalten keine Symbol-Glyphen) |

Daten pro Aufnahme liegen in `<data>/sessions/<zeitstempel>/` (`tuto-mic paths`).

## Datenschutz

Pseudonymisierte Daten sind nach DSGVO weiterhin personenbezogen. Für einen echten Einsatz an Schulen
braucht es u. a. Einwilligungen bzw. eine Rechtsgrundlage für die Aufnahme, eine Datenschutz-Folgenabschätzung
und – bei Cloud-Diensten – einen Auftragsverarbeitungsvertrag. Die App setzt technisch auf: lokale
Verarbeitung, sofortiges Löschen des Audios, keine Stimmprofile, Pseudonymisierung vor jedem KI-Schritt.

