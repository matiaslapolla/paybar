#!/usr/bin/env bash
# Install the paybar Omarchy bar widget.
#
# Symlinks rather than copies, so editing this repo hot-reloads the running
# shell — the plugin loader watches ~/.config/omarchy/plugins/.
set -euo pipefail

PLUGIN_ID="matiaslapolla.paybar"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/plugins/$PLUGIN_ID"

mkdir -p "$(dirname "$DEST")"

if [[ -e "$DEST" && ! -L "$DEST" ]]; then
  echo "error: $DEST exists and is not a symlink; remove it first" >&2
  exit 1
fi

ln -sfn "$SRC" "$DEST"
echo "linked $DEST -> $SRC"

if ! command -v omarchy-shell >/dev/null 2>&1; then
  echo "warning: omarchy-shell not found; open a new session to pick the plugin up" >&2
  exit 0
fi

omarchy-shell shell rescanPlugins >/dev/null
omarchy plugin enable "$PLUGIN_ID" >/dev/null 2>&1 \
  || omarchy-shell shell enablePlugin "$PLUGIN_ID" true >/dev/null
echo "enabled $PLUGIN_ID"

command -v paybar >/dev/null 2>&1 \
  || echo "note: paybar is not on PATH yet; the widget will say so until it is" >&2
