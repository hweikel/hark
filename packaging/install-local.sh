#!/usr/bin/env bash
# Put hark! where the desktop can find it, without a Nix package.
#
# The flake currently builds only a devShell, so `cargo tauri build` leaves the
# binary in src-tauri/target/release/ — a path no launcher knows about and that
# isn't on PATH. This script bridges that gap:
#
#   ~/.local/bin/hark                      symlink to the release build
#   ~/.local/share/applications/           the .desktop entry rofi -show drun reads
#   ~/.local/share/icons/hicolor/          the icon that entry names
#
# The symlink is deliberate: you are still iterating on this app, and a copy
# would mean every rebuild silently leaves the launcher running yesterday's
# binary. The tradeoff is that `cargo clean` breaks the launcher until the next
# build — re-run this script if that happens.
#
# Exec= is written as an absolute path rather than the bare `hark` the repo's
# .desktop uses, because ~/.local/bin is not on this machine's PATH and the i3
# session env is fixed at login — a PATH edit wouldn't reach rofi without a
# re-login. An absolute path works immediately.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$repo/src-tauri/target/release/hark"

bindir="$HOME/.local/bin"
appdir="$HOME/.local/share/applications"
icondir="$HOME/.local/share/icons/hicolor"

if [ ! -x "$binary" ]; then
  echo "no release binary at $binary" >&2
  echo "build it first:  cd '$repo' && nix develop --command cargo tauri build --no-bundle" >&2
  exit 1
fi

mkdir -p "$bindir" "$appdir"
ln -sfn "$binary" "$bindir/hark"

# Same entry as packaging/hark.desktop, with Exec resolved to an absolute path.
sed "s|^Exec=hark$|Exec=$bindir/hark|" "$repo/packaging/hark.desktop" \
  > "$appdir/hark.desktop"
chmod 644 "$appdir/hark.desktop"

for size in 32x32 128x128 256x256 512x512; do
  src="$repo/src-tauri/icons/$size.png"
  [ -f "$src" ] || continue
  mkdir -p "$icondir/$size/apps"
  install -m644 "$src" "$icondir/$size/apps/hark.png"
done

# Only speeds up lookup; both rofi and GTK fall back to scanning the dirs.
command -v gtk-update-icon-cache >/dev/null && \
  gtk-update-icon-cache -qtf "$icondir" 2>/dev/null || true
command -v update-desktop-database >/dev/null && \
  update-desktop-database -q "$appdir" 2>/dev/null || true

echo "installed:"
echo "  $bindir/hark -> $binary"
echo "  $appdir/hark.desktop"
echo "  $icondir/{32x32,128x128,256x256,512x512}/apps/hark.png"
echo
echo "rofi picks this up with no restart. For \`hark sync\` in a shell, add"
echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
echo "to your shell profile."
