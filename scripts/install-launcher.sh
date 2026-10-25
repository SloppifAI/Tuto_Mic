#!/usr/bin/env bash
# Legt im App-Menü „Tuto Mic“ (Ansicht der Lehrkraft) und „Tuto Mic (Admin)“ an. Beide starten über
# scripts/start.sh den letzten Stand von main.
set -euo pipefail
repo=$(git -C "$(dirname "$(readlink -f "$0")")" rev-parse --show-toplevel)
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
mkdir -p "$apps"
rm -f "$apps/tuto-mic-dev.desktop" # alter Eintrag

entry() { # Datei Name Kommentar Argumente
  cat > "$apps/$1" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$2
Comment=$3
Exec=$repo/scripts/start.sh $4
Icon=$repo/assets/images/icon.png
Categories=Education;
DESKTOP
  echo "Angelegt: $apps/$1"
}
entry tuto-mic-start.desktop "Tuto Mic" "Letzter Stand von main, Ansicht der Lehrkraft" ""
entry tuto-mic-admin.desktop "Tuto Mic (Admin)" "Letzter Stand von main, mit Einstellungen und Prompt" "--admin"
