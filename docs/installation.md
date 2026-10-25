# Tuto Mic installieren

Schritt für Schritt für den Rechner im Klassenraum. Dauer: etwa 15 Minuten, davon das meiste Downloads.

## 1. Ollama und Sprachmodell

Tuto Mic schreibt die Zusammenfassung mit einem Sprachmodell, das lokal über [Ollama](https://ollama.com) läuft.

1. Ollama von [ollama.com/download](https://ollama.com/download) installieren und starten (unter Windows und
   macOS läuft es danach im Hintergrund, Symbol im Infobereich bzw. in der Menüleiste).
2. In einem Terminal (Windows: „Eingabeaufforderung“ oder „PowerShell“) das Modell laden (mehrere GB):

   ```sh
   ollama pull qwen3.5:9b
   ```

Empfohlen ist eine Grafikkarte mit mindestens 8 GB Speicher; ohne läuft es auch, aber deutlich langsamer.

## 2. Tuto Mic herunterladen

Unter [Releases](https://github.com/SloppifAI/Tuto_Mic/releases) die Datei für das System laden:

| System | Datei |
|---|---|
| Windows 10/11 | `tuto-mic-windows-x86_64.zip` |
| macOS (Apple Silicon, z. B. Mac mini M1–M4) | `tuto-mic-macos-arm64.tar.gz` |
| Linux (x86_64) | `tuto-mic-linux-x86_64.tar.gz` |

Entpacken, z. B. nach `Dokumente\Tuto Mic` bzw. in den Programme-Ordner. Es gibt noch keinen Installer.

### Erster Start und Sicherheitswarnung

Die Programme sind noch nicht signiert, das System warnt deshalb einmalig:

- **Windows:** Doppelklick auf `tuto-mic.exe` → „Der Computer wurde durch Windows geschützt“ →
  **Weitere Informationen** → **Trotzdem ausführen**. Neben dem Fenster öffnet sich ein schwarzes
  Konsolenfenster mit Protokollausgaben – das ist normal, bitte offen lassen.
- **macOS:** Rechtsklick auf `tuto-mic` → **Öffnen** → **Öffnen**. Bietet macOS das nicht an: nach dem ersten
  Versuch unter *Systemeinstellungen → Datenschutz & Sicherheit* **Dennoch öffnen**. Alternativ im Terminal:
  `xattr -d com.apple.quarantine tuto-mic`.
- **Linux:** `./tuto-mic` im entpackten Ordner. Nötig ist `libvulkan.so.1` (Paket `libvulkan1`).

Beim ersten Zugriff auf das Mikrofon fragt das System nach der Erlaubnis – zustimmen.

## 3. Einrichten (einmalig)

1. **Passwort festlegen.** Tuto Mic verschlüsselt damit Einstellungen und Klassenliste. Es wird bei jedem Start
   gebraucht und lässt sich **nicht wiederherstellen**. Es sollten nur die Betreuung bzw. die Lehrkräfte kennen.
2. **Spracherkennung herunterladen.** Knopf auf der Startseite, ca. 550 MB. Danach funktioniert die
   Transkription ohne Internet.
3. **Mit Tuto verbinden.** Oben rechts „Mit Tuto verbinden“ → im Browser bei Tuto anmelden → **Erlauben** → die
   Rückfrage des Browsers („Tuto Mic öffnen?“) bestätigen. Oben rechts steht dann der eigene Name.
   - Unter **macOS** springt der Browser noch nicht automatisch zurück; die Adresse, die mit
     `tutoapp://callback` beginnt, unter „Kommt nichts zurück?“ einfügen.
   - Die Namen der Klassen kommen ab dann aus Tuto (nur im Arbeitsspeicher, nicht gespeichert).

## 4. Benutzen

1. **Für welche Stunde?** – ist mit der Stunde aus dem Tuto-Stundenplan vorbelegt; bei Vertretung ändern.
2. **Aufnahme starten** (roter Knopf). Pause ist möglich; „Letzte Minuten verwerfen“ löscht das Ende der Aufnahme.
3. **Beenden und auswerten.** Die Stunde wurde schon während der Aufnahme mitgeschrieben; danach dauert es meist
   noch etwa eine Minute für die Zusammenfassung.
4. **Zusammenfassung prüfen**, bei Bedarf direkt im Textfeld ändern.
5. **Zusammenfassung senden.** Danach werden die Daten der Stunde auf dem Rechner gelöscht.

## Für Betreuung und Tests

- **Admin-Modus:** `tuto-mic --admin` (Windows: Verknüpfung mit dem Zusatz `--admin` anlegen) zeigt ein Menü mit
  Einstellungen (Klassenliste, Modelle, Tuto-Adresse), Prompt, Transkript und Log sowie
  „Audio- oder Textdatei verarbeiten …“ zum Ausprobieren ohne Mikrofon.
- **Zustand prüfen:** `tuto-mic info` im Terminal zeigt GPU, Modelle und Konfiguration (fragt nach dem Passwort).
- Weitere Einstellungen über Umgebungsvariablen: siehe [README, Für Schul-IT](../README.md#für-schul-it-und-betreuung).

## Passwort zurücksetzen (nur Betreuung)

In der App gibt es dafür bewusst keinen Knopf: Sonst könnte jemand ohne Passwort ein eigenes festlegen und
Einstellungen oder Prompt ändern. Wer Zugriff auf das Benutzerkonto des Rechners hat, löscht bei geschlossener
App diese Dateien (Ordner zeigt `tuto-mic paths` unter „Konfiguration“):

- `config.salt`
- `config.json.enc`
- `prompts/summary.md.enc` (falls vorhanden)

Beim nächsten Start wird ein neues Passwort festgelegt; Einstellungen, lokale Klassenliste und eigener Prompt sind
dann auf den Standardwerten. Ordner: Windows `%APPDATA%\Tuto\tuto-mic\config\`, macOS
`~/Library/Application Support/de.Tuto.tuto-mic/`, Linux `~/.config/tuto-mic/`.

## Häufige Probleme

| Problem | Lösung |
|---|---|
| „Es konnte keine Zusammenfassung erstellt werden“ | Läuft Ollama? `ollama list` muss `qwen3.5:9b` zeigen. |
| Transkription sehr langsam | `tuto-mic info` zeigt, ob eine GPU gefunden wurde. Grafiktreiber aktualisieren. |
| Nach „Erlauben“ passiert nichts | Rückfrage des Browsers bestätigen; Tuto Mic darf nur einmal laufen. Notfalls Adresse einfügen (siehe oben). |
| Windows: Programm startet nicht, Meldung zu `vulkan-1.dll` | Aktuellen Grafiktreiber des Herstellers installieren. |
| Passwort vergessen | Nur die Betreuung kann zurücksetzen: siehe „Passwort zurücksetzen“ oben. |

## Download prüfen (optional)

Jedes Release enthält `SHA256SUMS.txt`; bei öffentlichem Repository zusätzlich einen signierten
Herkunftsnachweis (GitHub Build-Provenance):

```sh
sha256sum -c SHA256SUMS.txt --ignore-missing     # macOS: shasum -a 256 -c SHA256SUMS.txt
gh attestation verify tuto-mic-windows-x86_64.zip --repo SloppifAI/Tuto_Mic
```

Windows (PowerShell): `Get-FileHash tuto-mic-windows-x86_64.zip -Algorithm SHA256` und mit der Zeile in
`SHA256SUMS.txt` vergleichen.
