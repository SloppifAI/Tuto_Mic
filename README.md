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
- **Admin-Modus** (`--admin` oder `TUTO_MIC_DEV=1`): Prompt-Editor, Audiodatei oder Text-Transkript importieren,
  Original-Transkript, Log

## Nutzung

```sh
tuto-mic                 # GUI
tuto-mic --admin         # GUI mit Admin-Menü: Einstellungen, Prompt, Transkript/Log, Datei-Import
tuto-mic download        # Whisper- und VAD-Modell laden (~550 MB, alternativ per Button in der GUI)
tuto-mic process stunde.mp3 [--no-llm]   # ohne GUI, z. B. zum Prompt-Testen
tuto-mic process stunde.txt              # fertiges Transkript: ohne Whisper direkt pseudonymisieren und zusammenfassen
tuto-mic paths           # Pfade von Konfiguration, Prompts, Sessions, Modellen
tuto-mic info            # Version, Whisper-Backend (GPU oder CPU), Modelle
```

`process` entscheidet nach der Endung: `.txt` und `.md` gelten als Text-Transkript (UTF-8, z. B. Demo-Daten),
alles andere als Audiodatei. Für Transkripte braucht es kein Whisper-Modell; in `export.json` sind dann
`audio_seconds` und `models.transcription` `null`.

## Konfiguration

Lehrkräfte sehen keine Einstellungen. Es gelten Standardwerte; Betreuer ändern sie zentral:

- Datei `config.json` (Pfad: `tuto-mic paths`, oder per `TUTO_MIC_CONFIG=/pfad/config.json` z. B. für alle Nutzer)
- Umgebungsvariablen `TUTO_MIC_*`, die Vorrang vor der Datei haben

`tuto-mic info` zeigt die gültigen Werte und alle Variablen (Tuto-Adresse, Höchstlänge, Ollama, Modelle,
Klassenliste …).

### An Tuto senden

Im Tab **Ergebnis** verbindet „Mit Tuto verbinden“ die App mit dem Tuto-Konto der Lehrkraft (Anmeldung im
Browser, OAuth mit PKCE – die App sieht kein Passwort). Danach lässt sich eine Stunde von heute wählen oder
anlegen und die Zusammenfassung senden (höchstens 5000 Zeichen; der Text ist vorher bearbeitbar). Der Zugang
liegt im Schlüsselbund des Betriebssystems.

Nach „Erlauben“ springt der Browser über `tutoapp://callback` zurück in die App (unter Linux und Windows
registriert sie sich dafür selbst). Klappt das nicht, die Adresse aus der Adresszeile in die App einfügen.

```sh
tuto-mic tuto login                       # dasselbe ohne GUI
tuto-mic tuto lessons [--date 2026-10-23]
tuto-mic tuto send <session-ordner> --lesson <id>
tuto-mic tuto status | logout
```

Die Tuto-Adresse steht in der Konfiguration (`tuto_url`).

Voraussetzung für die Zusammenfassung: [Ollama](https://ollama.com) läuft lokal und hat ein Modell,
z. B. `ollama pull qwen3.5:9b`. Ohne Ollama werden Transkript und pseudonymisiertes Transkript trotzdem erstellt.

### Prompt anpassen

Der Zusammenfassungs-Prompt `prompts/summary.md` ist in die App eingebaut. Zum Testen lässt er sich überschreiben:

- im Admin-Modus über den Tab **Prompts** (speichert nach `<config>/prompts/`), oder
- per Verzeichnis: `TUTO_MIC_PROMPTS=./meine-prompts tuto-mic process stunde.mp3`
  (Datei `summary.md`; fehlt sie, gilt die eingebaute Version).

Das Transkript (`[mm:ss] text` pro Zeile; importierte Text-Transkripte unverändert) ersetzt den Platzhalter
`{{TRANSKRIPT}}`; fehlt er, wird es angehängt.

## Entwicklung

Mit Nix (empfohlen, enthält Rust, rust-analyzer, cmake, clang, die GUI-/Audio-Bibliotheken und die Prüfwerkzeuge).
Mit direnv lädt `.envrc` die Umgebung automatisch, auch für den Editor (`direnv allow`).

```sh
nix develop                  # Standard: Whisper auf der GPU per Vulkan
nix develop .#cuda           # zusätzlich CUDA, dann: cargo run --release --features cuda
cargo run -- --admin
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
`libasound2-dev libxkbcommon-dev libwayland-dev libvulkan-dev glslc` (Ubuntu ab 24.04; älter: Vulkan-SDK
von LunarG), unter Windows das [Vulkan-SDK](https://vulkan.lunarg.com/sdk/home) mit gesetztem `VULKAN_SDK`.

#### GPU

Whisper nutzt die GPU ohne Feature-Flag, auch in den Release-Paketen:

- **Linux, Windows:** Vulkan – läuft mit den normalen Treibern von NVIDIA, AMD und Intel.
- **macOS:** Metal.
- **`--features cuda`** (optional, nur NVIDIA, braucht das CUDA-Toolkit, unter Nix `nix develop .#cuda`):
  zusätzlich CUDA, schneller als Vulkan. ggml wählt dann zuerst das CUDA-Gerät.

Findet whisper.cpp keine passende GPU, rechnet es auf der CPU – das funktioniert, ist für 45 min Unterricht
aber deutlich langsamer. Was tatsächlich genutzt wird, zeigt `tuto-mic info` und jede `log.txt`
(z. B. `Whisper: Vulkan (GPU: Vulkan0)`).

Messung (RTX 3080 Laptop, large-v3-turbo, Beam 5): 33 min Unterricht mit Vulkan in 64 s, mit CUDA in
etwa 66 s; 2,5 min Audio mit Vulkan in 5–6 s, auf der CPU (i7, ausgelastete Maschine) nach 16 min erst zu
31 % fertig.

Auf Laptops mit zwei GPUs nimmt Tuto Mic die dedizierte statt der integrierten GPU (`src/gpu.rs`).

Die Linux- und Windows-Programme brauchen zur Laufzeit die Vulkan-Bibliothek (`libvulkan.so.1` bzw.
`vulkan-1.dll`). Sie gehört zu jedem aktuellen Grafiktreiber; fehlt sie (z. B. Server oder VM ohne
Grafiktreiber), startet das Programm nicht (Linux: Paket `libvulkan1` installieren).

### Builds

GitHub Actions:

- `ci.yml`: bei jedem Push und Pull Request Formatierung, Lints, Tests und Abhängigkeitsprüfung (nur Linux).
- `release.yml`: Builds für Linux und Windows (Vulkan) sowie macOS (Apple Silicon, Metal). Ein Tag wie `v0.1.0` erzeugt ein
  GitHub-Release mit den Paketen zum Download; von Hand gestartet („Run workflow“) nur Test-Artefakte.

```sh
git tag v0.1.0 && git push origin v0.1.0   # Release erstellen
```

Noch offen: Signierung (macOS-Notarisierung, Windows-Code-Signing), Installer.

### Aufbau

| Modul | Aufgabe |
|---|---|
| `src/main.rs`, `src/lib.rs` | Einstieg (CLI/GUI) bzw. Bibliothek mit der gesamten Logik |
| `src/app.rs` | egui-Oberfläche |
| `src/control.rs` | Befehle (Start/Pause/Stop/Löschen) und Aufnahmezustand – Schnittstelle für späteren Hardware-Taster |
| `src/recorder.rs` | Mikrofon (cpal), Resampling, Segment-Dateien, rückwirkendes Löschen |
| `src/audio.rs` | Audio laden/dekodieren (symphonia), Resampling auf 16 kHz (rubato) |
| `src/transcribe.rs` | whisper.cpp über whisper-rs |
| `src/gpu.rs` | Whisper-Backend: eingebaut (Vulkan/Metal/CUDA) und zur Laufzeit gewählt (GPU oder CPU) |
| `src/anonymize.rs` | Pseudonymisierung |
| `src/llm.rs` | Ollama-HTTP-API |
| `src/tuto/`, `src/tuto_panel.rs` | Tuto-Anbindung: Anmeldung (PKCE), Schnittstelle, Token-Ablage, CLI bzw. GUI |
| `src/deeplink.rs` | `tutoapp://`-Links registrieren und an die laufende App weitergeben |
| `src/pipeline.rs` | Ablauf nach der Aufnahme, Export |
| `src/prompts.rs`, `prompts/` | eingebauter + überschreibbarer Prompt |
| `src/icons.rs` | selbst gezeichnete Icons (egui-Schriften enthalten keine Symbol-Glyphen) |

Daten pro Aufnahme liegen in `<data>/sessions/<zeitstempel>/` (`tuto-mic paths`).

## Datenschutz

Pseudonymisierte Daten sind nach DSGVO weiterhin personenbezogen. Für einen echten Einsatz an Schulen
braucht es u. a. Einwilligungen bzw. eine Rechtsgrundlage für die Aufnahme, eine Datenschutz-Folgenabschätzung
und – bei Cloud-Diensten – einen Auftragsverarbeitungsvertrag. Die App setzt technisch auf: lokale
Verarbeitung, sofortiges Löschen des Audios, keine Stimmprofile, Pseudonymisierung vor jedem KI-Schritt.

