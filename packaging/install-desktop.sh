#!/usr/bin/env bash
# Put C-Shop in the desktop's application menu.
#
# Everything goes under $XDG_DATA_HOME (~/.local/share by default), so this
# needs no root and touches nothing outside the user's own account. Run it
# again after moving the binary; run it with --uninstall to undo it.

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
apps="$data/applications"
icons="$data/icons/hicolor"
mime="$data/mime"

desktop_file="$apps/cshop.desktop"
mime_file="$mime/packages/cshop.xml"
sizes=(16 24 32 48 64 128 256 512)

# The formats the editor actually opens. PSD is claimed through `image/x-psd`,
# a registered alias of the canonical type, which the mime database resolves.
mimetypes="image/png;image/jpeg;image/bmp;image/gif;image/tiff;image/webp;image/x-tga;image/vnd.microsoft.icon;image/x-psd;application/x-cshop-project;"

usage() {
    cat <<'USAGE'
usage: install-desktop.sh [options]

  --uninstall     remove everything this installed
  --no-desktop    only add it to the application menu, not to ~/Desktop
  --binary PATH   use this binary instead of ./target/release/cshop
  -h, --help      this message
USAGE
}

uninstall=false
put_on_desktop=true
binary=""

while [ $# -gt 0 ]; do
    case "$1" in
        --uninstall)   uninstall=true ;;
        --no-desktop)  put_on_desktop=false ;;
        --binary)      shift; binary="${1:-}" ;;
        -h|--help)     usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 1 ;;
    esac
    shift
done

refresh_caches() {
    # Each of these is optional; a desktop without them still works, it just
    # notices the new entry a little later.
    command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
    command -v gtk-update-icon-cache   >/dev/null && gtk-update-icon-cache -qtf "$icons" 2>/dev/null || true
    command -v update-mime-database    >/dev/null && update-mime-database "$mime" 2>/dev/null || true
}

desktop_dir() {
    # Honours a localised or relocated Desktop directory rather than assuming.
    if command -v xdg-user-dir >/dev/null; then
        xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop"
    else
        echo "$HOME/Desktop"
    fi
}

if [ "$uninstall" = true ]; then
    rm -f "$desktop_file" "$mime_file" "$(desktop_dir)/cshop.desktop"
    for size in "${sizes[@]}"; do
        rm -f "$icons/${size}x${size}/apps/cshop.png"
    done
    refresh_caches
    echo "removed C-Shop from the application menu"
    exit 0
fi

# The binary, which has to exist before anything points at it.
if [ -z "$binary" ]; then
    binary="$repo/target/release/cshop"
    if [ ! -x "$binary" ]; then
        echo "no release binary yet — building it"
        ( cd "$repo" && cargo build --release --bin cshop )
    fi
fi
if [ ! -x "$binary" ]; then
    echo "not an executable: $binary" >&2
    exit 1
fi
binary="$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")"

mkdir -p "$apps" "$mime/packages"
for size in "${sizes[@]}"; do
    mkdir -p "$icons/${size}x${size}/apps"
    install -m 644 "$repo/packaging/icons/cshop-${size}.png" "$icons/${size}x${size}/apps/cshop.png"
done

install -m 644 "$repo/packaging/cshop-mime.xml" "$mime_file"

# Absolute path rather than relying on the binary being on PATH: a desktop
# session's PATH is not the shell's, and ~/.local/bin is often missing from it.
sed -e "s|@EXEC@|$binary|g" -e "s|@MIMETYPES@|$mimetypes|g" \
    "$repo/packaging/cshop.desktop.in" > "$desktop_file"
chmod 644 "$desktop_file"

if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate "$desktop_file" || {
        echo "the generated entry did not validate; leaving it in place to inspect" >&2
        exit 1
    }
fi

refresh_caches

if [ "$put_on_desktop" = true ]; then
    target="$(desktop_dir)"
    if [ -d "$target" ]; then
        install -m 755 "$desktop_file" "$target/cshop.desktop"
        # GNOME will not run a launcher it does not trust, and shows the file
        # name instead of the icon until it does.
        if command -v gio >/dev/null; then
            gio set "$target/cshop.desktop" metadata::trusted true 2>/dev/null || true
        fi
        echo "placed a launcher in $target"
    else
        echo "no desktop directory found; skipped the desktop icon"
    fi
fi

echo "installed:"
echo "  menu entry  $desktop_file"
echo "  runs        $binary"
echo "  icons       $icons/{$(IFS=,; echo "${sizes[*]}")}x…/apps/cshop.png"
echo "  file types  .cshop .psd and the usual images"
