#!/usr/bin/env bash
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
BIN_NAME="spark"
APP_NAME="Spark"
DESKTOP_DIR="${DESKTOP_DIR:-$HOME/.local/share/applications}"
DESKTOP_FILE="${DESKTOP_FILE:-spark.desktop}"
FORCE_X11="${FORCE_X11:-}"

EXEC_CMD="$BIN_DIR/$BIN_NAME"
TERMINAL_ENTRY="true"
STARTUP_WMCLASS_LINE=""

if ! command -v rustc >/dev/null 2>&1; then
  if command -v apt >/dev/null 2>&1; then
    echo "rustc not found; installing via apt..."
    sudo apt install -y rustc
  else
    echo "rustc not found and apt is unavailable. Please install Rust first."
    exit 1
  fi
fi

if command -v gnome-terminal >/dev/null 2>&1; then
  SESSION_BACKEND=""
  if [ -n "${GDK_BACKEND:-}" ]; then
    SESSION_BACKEND="$GDK_BACKEND"
  elif [ -n "${WAYLAND_DISPLAY:-}" ] || [ "${XDG_SESSION_TYPE:-}" = "wayland" ]; then
    SESSION_BACKEND="wayland"
  elif [ -n "${DISPLAY:-}" ] || [ "${XDG_SESSION_TYPE:-}" = "x11" ]; then
    SESSION_BACKEND="x11"
  fi
  if [ -z "$FORCE_X11" ] && [ "$SESSION_BACKEND" = "wayland" ]; then
    FORCE_X11="1"
  fi
  if [ "${FORCE_X11}" = "1" ] || [ "${FORCE_X11}" = "true" ]; then
    EXEC_CMD="env GDK_BACKEND=x11 gnome-terminal --class=${APP_NAME} --name=${APP_NAME} -- $BIN_DIR/$BIN_NAME"
  else
    EXEC_CMD="gnome-terminal --class=${APP_NAME} --name=${APP_NAME} -- $BIN_DIR/$BIN_NAME"
  fi
  TERMINAL_ENTRY="false"
  STARTUP_WMCLASS_LINE="StartupWMClass=$APP_NAME"
fi

echo "Building release binary..."
cargo build --release

echo "Installing to $BIN_DIR/$BIN_NAME"
install -d "$BIN_DIR"
install -m 755 "target/release/$BIN_NAME" "$BIN_DIR/$BIN_NAME"

echo "Installing desktop entry to $DESKTOP_DIR/$DESKTOP_FILE"
install -d "$DESKTOP_DIR"
rm -f "$DESKTOP_DIR/spark.desktop" \
  "$DESKTOP_DIR/${BIN_NAME}.desktop"
{
  echo "[Desktop Entry]"
  echo "Type=Application"
  echo "Name=$APP_NAME"
  echo "Comment=Terminal task manager"
  echo "Exec=$EXEC_CMD"
  echo "Terminal=$TERMINAL_ENTRY"
  echo "Icon=utilities-system-monitor"
  echo "Categories=System;Monitor;"
  if [ -n "$STARTUP_WMCLASS_LINE" ]; then
    echo "$STARTUP_WMCLASS_LINE"
  fi
} > "$DESKTOP_DIR/$DESKTOP_FILE"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true
fi

echo "Done."
