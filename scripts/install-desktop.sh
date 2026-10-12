#!/bin/sh
# Install desktop identity only; the shotori executable must already be on PATH.
# Pass /usr for system packages, or use DESTDIR to stage a package safely.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$#" -gt 1 ]; then
    echo "Usage: $0 [prefix]" >&2
    exit 2
fi
prefix=${1:-${HOME}/.local}
destination=${DESTDIR:-}${prefix}/share
install -Dm644 "$root/assets/shotori.desktop" "$destination/applications/shotori.desktop"
install -Dm644 "$root/assets/app/shotori.svg" "$destination/icons/hicolor/scalable/apps/shotori.svg"
for source in "$root"/assets/app/shotori-*.png; do
    size=${source##*/shotori-}
    size=${size%.png}
    install -Dm644 "$source" "$destination/icons/hicolor/${size}x${size}/apps/shotori.png"
done
# Package managers refresh caches themselves after staging/installing packages.
if [ -z "${DESTDIR:-}" ]; then
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -f -t "$destination/icons/hicolor"
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$destination/applications"
    fi
fi
