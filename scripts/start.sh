#!/usr/bin/env bash
# Startet Tuto Mic im Stand des letzten Commits auf `main` – in einem eigenen Git-Worktree mit eigenem Build.
# So stören halbfertige Änderungen im Arbeitsverzeichnis den Start nicht, und es gibt keine Wartezeit durch
# gesperrte Build-Verzeichnisse. Erster Start: einmalig ca. 10 min (whisper.cpp), danach Sekunden.
#
#   scripts/start.sh --admin      # mit Einstellungen, Prompt-Editor und Rohdaten
#   scripts/start.sh info         # jeder andere tuto-mic-Befehl geht auch
set -euo pipefail

repo=$(git -C "$(dirname "$(readlink -f "$0")")" rev-parse --show-toplevel)
run="${TUTO_MIC_START_DIR:-$repo/../Tuto_Mic-start}"
log="${XDG_CACHE_HOME:-$HOME/.cache}/tuto-mic-start.log"
mkdir -p "$(dirname "$log")"

notify() { command -v notify-send >/dev/null && notify-send -a "Tuto Mic" "Tuto Mic" "$1" || true; }

if [ ! -d "$run" ]; then
  git -C "$repo" worktree add --quiet --detach "$run" main
fi
git -C "$run" checkout --quiet --detach main

cd "$run"
# Bauen getrennt vom Start, damit ein Build-Fehler sichtbar wird statt eines stummen Abbruchs.
if ! nix develop --quiet -c cargo build --release --quiet 2>"$log"; then
  notify "Build fehlgeschlagen, siehe $log"
  cat "$log" >&2
  exit 1
fi
exec nix develop --quiet -c ./target/release/tuto-mic "$@"
