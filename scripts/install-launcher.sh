#!/usr/bin/env bash
# Legt einen Eintrag „Tuto Mic (Entwicklung)“ im App-Menü an, der scripts/start.sh --dev startet.
set -euo pipefail
repo=$(git -C "$(dirname "$(readlink -f "$0")")" rev-parse --show-toplevel)
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
mkdir -p "$apps"
cat > "$apps/tuto-mic-dev.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Tuto Mic (Entwicklung)
Comment=Letzter Stand von main, mit Dev-Modus
Exec=$repo/scripts/start.sh --dev
Icon=$repo/assets/images/icon.png
Categories=Development;Education;
DESKTOP
echo "Angelegt: $apps/tuto-mic-dev.desktop"
