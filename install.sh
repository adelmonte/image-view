#!/bin/sh
# Build and install image-view for the current user (PREFIX=/usr/local sudo ./install.sh for system-wide)
set -e
PREFIX="${PREFIX:-$HOME/.local}"
cargo build --release
install -Dm755 target/release/image-view "$PREFIX/bin/image-view"
install -Dm644 dist/image-view.desktop "$PREFIX/share/applications/image-view.desktop"
for d in dist/hicolor/*/apps; do
  size=$(basename "$(dirname "$d")")
  for f in "$d"/*; do
    ext="${f##*.}"; case "$f" in *symbolic*) name=image-view-symbolic;; *) name=image-view;; esac
    install -Dm644 "$f" "$PREFIX/share/icons/hicolor/$size/apps/$name.$ext"
  done
done
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
echo "Installed to $PREFIX/bin/image-view"
