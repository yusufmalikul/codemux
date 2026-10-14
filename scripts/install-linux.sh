#!/bin/sh
# Build Codemux and install it for the current user,
# so it opens from the app menu of GNOME, KDE and other desktops.
# Files go to ~/.local (no sudo needed).
set -eu
cd "$(dirname "$0")/.."

cargo build --release -p codemux

data="${XDG_DATA_HOME:-$HOME/.local/share}"
bin="$HOME/.local/bin"
mkdir -p "$bin" "$data/applications" "$data/icons/hicolor/256x256/apps"

install -m 755 target/release/Codemux "$bin/codemux"
install -m 644 crates/app/assets/icon-256.png "$data/icons/hicolor/256x256/apps/codemux.png"

# The file name must match the window's app id ("codemux"),
# so Wayland desktops show the right icon in the dock.
cat > "$data/applications/codemux.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Codemux
Comment=Session manager for Claude Code
Exec=$bin/codemux
Icon=codemux
Terminal=false
Categories=Development;
StartupWMClass=codemux
DESKTOP

command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$data/icons/hicolor" || true
echo "Installed $bin/codemux and the Codemux menu entry"
