# Hinweise für KI-Agenten

Tuto Mic nimmt Unterricht auf, transkribiert lokal (whisper.cpp), pseudonymisiert und fasst mit einem lokalen
LLM (Ollama) zusammen. Rust, Desktop-GUI mit egui. Überblick und Modultabelle: `README.md`.

## Befehle

Alles läuft in der Nix-Devshell (`nix develop`, oder automatisch per direnv):

```sh
cargo test                                   # alle Tests, ohne Modelle, Mikrofon oder Ollama
cargo clippy --all-targets -- -D warnings    # Lints (Regeln in Cargo.toml)
cargo fmt                                    # Formatierung (rustfmt.toml)
cargo deny check                             # Sicherheitslücken, Lizenzen (deny.toml)
cargo machete                                # ungenutzte Abhängigkeiten
cargo test ui_preview -- --ignored           # alle GUI-Zustände als PNG nach target/ui-preview/ (braucht GPU)
cargo app --dev                              # App (Release-Build, Whisper per Vulkan/Metal auf der GPU)
cargo app info                               # zeigt, ob Whisper die GPU findet
```

## Wartezeiten vermeiden

- Jede neue Kombination aus Profil und Features (Debug/Release × CUDA/Vulkan) übersetzt whisper.cpp neu
  (~10 min). Nur zwei Kombinationen verwenden: Prüfungen (`cargo test`/`clippy`) und die App über `cargo app`
  (Release), beide in `nix develop`. CUDA (`nix develop .#cuda`, `--features cuda`) nur bei Bedarf.
- Praxistests mit kurzen Clips (20 s bis 3 min) oder Text-Transkripten; ganze Stunden erst zur Abschlussprüfung.
- Zusätzliche Git-Worktrees (z. B. Subagenten) mit `CARGO_TARGET_DIR` auf das `target/` des Hauptverzeichnisses
  bauen, sonst übersetzt jeder whisper.cpp erneut.

Vor dem Abschluss einer Aufgabe müssen `fmt`, `clippy` und `test` ohne Befund durchlaufen; die CI prüft
dasselbe plus `deny` und `machete`.

## Datenschutz (wichtigste Regel)

- Keine echten Namen von Schüler:innen oder Lehrkräften in Code, Tests, Prompts, Logs oder Commits.
  In Tests fiktive Namen verwenden (z. B. „Lena Wagner“).
- Keine Aufnahmen oder Transkripte echter Stunden ins Repository.
- Pseudonymisierung ist deterministischer Code (`src/anonymize.rs`) und läuft vor jedem LLM-Aufruf. Sie wird
  nie einem LLM überlassen: Lokale Modelle haben im Test Namen trotz Verbot im Prompt ausgegeben.
- Nur pseudonymisierte Daten verlassen die Pipeline (`export.json`, Zusammenfassung). Neue Ausgaben brauchen
  einen Test, der das prüft (Beispiel: `pipeline::tests`).

## Code

- Bezeichner auf Englisch; Kommentare, Doku, Fehlermeldungen und GUI-Texte auf Deutsch.
- Kommentare erklären das Warum, nicht das Was. Die Kommentardichte des umgebenden Codes übernehmen.
- Fehler mit `anyhow`, mit `.context(…)`/`.with_context(…)` und einer Meldung, die Nutzer:innen verstehen.
- Kein `.unwrap()` außerhalb von Tests; stattdessen `?` oder `.expect("warum das nicht fehlschlagen kann")`.
  Mutex: `.lock().unwrap_or_else(PoisonError::into_inner)`.
- Lints in `Cargo.toml` nicht lockern. Einzelne Ausnahmen mit `#[expect(clippy::…, reason = "…")]`.
- Logik gehört in die Bibliothek (`src/lib.rs`), nicht in `main.rs` oder die GUI, damit sie ohne Fenster
  testbar bleibt. `src/app.rs` ruft nur auf und zeigt an.
- GUI-Änderungen mit `cargo test ui_preview -- --ignored` ansehen, bevor jemand die App startet. Jedes
  Bedienelement braucht einen zugänglichen Namen (die Vorschau prüft das).
- egui-Schriften enthalten keine Symbole (●, ▶, ⚠, →, …): Icons über `src/icons.rs` zeichnen, sonst Text.
- Neue Abhängigkeiten nur nach Rückfrage; ihre Lizenz muss in `deny.toml` erlaubt sein.

## Tests

- Neue oder geänderte Logik bekommt Tests im selben Modul (`#[cfg(test)] mod tests`).
- Netzwerk: `crate::test_http::serve_once` statt echter Server. Dateien: `tempfile::tempdir()`.
- Tests dürfen keine Modelle, kein Mikrofon und kein laufendes Ollama voraussetzen.

## Bibliotheken

Die APIs ändern sich zwischen Versionen stark (egui 0.37, cpal 0.18, symphonia 0.6, rubato 5, whisper-rs 0.16,
ureq 3). Vor der Nutzung die Version in `Cargo.toml` prüfen und im Zweifel den Quelltext unter
`~/.cargo/registry/src/` lesen, statt sich auf ältere Beispiele zu verlassen.

## Prompt

`prompts/summary.md` ist der Zusammenfassungs-Prompt des Teams, mit dem bereits getestet wurde. Den Kern
(Regeln 1–3, Gliederung) beibehalten. `{{TRANSKRIPT}}` markiert die Stelle für das Transkript. Tuto nimmt
Zusammenfassungen bis 5000 Zeichen an.
