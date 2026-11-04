#!/usr/bin/env bash
# Build Pantheon and add it to your application menu, under ~/.local only.
#
#   scripts/install-desktop.sh               build, then install both menu entries
#   scripts/install-desktop.sh --no-build    install entries for binaries already built
#   scripts/install-desktop.sh --no-3d       Pantheon only, without Pantheon 3D
#   scripts/install-desktop.sh --uninstall   remove the menu entries and icons
#
# The entries point at the binaries in this checkout, which is also where the
# datasets live, so moving or deleting the checkout breaks them. Run this again
# after moving it. It refuses a checkout path with $ ` " \ or % in it, because a
# menu entry can't name such a path reliably.
#
# It doesn't overwrite what it didn't write: a menu entry of the same name, or an icon
# of the same name that differs from ours with no entry of ours beside it, stops the
# install. Uninstalling removes only our entries and icons, then refreshes the shared
# menu and icon caches so they stop listing Pantheon. The caches and folders stay, and
# so do the checkout, its downloads, and your settings in ~/.config/pantheon.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
apps="$HOME/.local/share/applications"
hicolor="$HOME/.local/share/icons/hicolor"
icons="$hicolor/256x256/apps"

# Pantheon 3D's entry is named after the window's app id (identifier in tauri.conf.json):
# on Wayland that name is how the desktop finds the window's icon and launcher.
#
# An entry is ours if it runs a Pantheon binary from a checkout's build folders, the
# only Exec line this script writes. That still matches after the checkout has moved.
our_entry() {
  grep -qE '^Exec=".*/target(-3d)?/release/pantheon(-3d)?"$' "$1"
}

remove_entry() {
  if [ ! -e "$1" ]; then
    return 0
  elif our_entry "$1"; then
    rm -- "$1"
  else
    echo "Left in place, not written by this script: $1"
  fi
}

# An icon is ours for removal only if it still matches the file in this checkout.
remove_icon() {
  if [ ! -e "$1" ]; then
    return 0
  elif cmp -s "$1" "$2"; then
    rm -- "$1"
  else
    echo "Left in place, differs from this checkout's icon: $1"
  fi
}

refuse_foreign_entry() {
  if [ -e "$1" ] && ! our_entry "$1"; then
    echo "$1 exists and wasn't written by this script." >&2
    echo "Move it away, then run this again." >&2
    exit 1
  fi
}

# An icon that differs from ours is an older one of ours if our entry is beside it.
# With no entry of ours, it belongs to something else.
refuse_foreign_icon() {
  if [ -e "$1" ] && ! cmp -s "$1" "$2" && ! { [ -e "$3" ] && our_entry "$3"; }; then
    echo "$1 exists and wasn't written by this script." >&2
    echo "Move it away, then run this again." >&2
    exit 1
  fi
}

build=1
with_3d=1
uninstall=0
for arg in "$@"; do
  case "$arg" in
    --no-build) build=0 ;;
    --no-3d) with_3d=0 ;;
    --uninstall) uninstall=1 ;;
    -h | --help)
      sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown option: $arg (try --help)" >&2
      exit 2
      ;;
  esac
done

refresh_caches() {
  if [ -d "$apps" ] && command -v update-desktop-database >/dev/null; then
    update-desktop-database "$apps" || true
  fi
  # -f because it only compares the top two folder levels' times, so it would call the
  # cache up to date after an icon goes from 256x256/apps
  if [ -d "$hicolor" ] && command -v gtk-update-icon-cache >/dev/null; then
    gtk-update-icon-cache -q -f -t "$hicolor" 2>/dev/null || true
  fi
}

if [ "$uninstall" = 1 ]; then
  remove_entry "$apps/pantheon.desktop"
  remove_entry "$apps/app.pantheon.atlas3d.desktop"
  remove_icon "$icons/pantheon.png" "$repo/assets/icon.png"
  remove_icon "$icons/pantheon-3d.png" "$repo/assets/icon-3d.png"
  refresh_caches
  echo "Removed the Pantheon menu entries and icons. Your checkout and settings are untouched."
  exit 0
fi

# A .desktop file can't name these reliably: Exec and Path treat them as expansions or
# escapes, so the entry would be invalid or run something else.
case "$repo" in
  *'$'* | *'`'* | *'"'* | *'\'* | *'%'* | *$'\n'*)
    echo "This checkout's path has a character a menu entry can't name: $repo" >&2
    echo "Move it somewhere without \$ \` \" \\ or %, then run this again." >&2
    exit 1
    ;;
esac

command -v cargo >/dev/null || {
  echo "cargo was not found. Install Rust from https://rustup.rs, then run this again." >&2
  exit 1
}

# Before any build, so a clash doesn't cost a long compile first
refuse_foreign_entry "$apps/pantheon.desktop"
refuse_foreign_entry "$apps/app.pantheon.atlas3d.desktop"
refuse_foreign_icon "$icons/pantheon.png" "$repo/assets/icon.png" "$apps/pantheon.desktop"
if [ "$with_3d" = 1 ]; then
  refuse_foreign_icon "$icons/pantheon-3d.png" "$repo/assets/icon-3d.png" "$apps/app.pantheon.atlas3d.desktop"
fi

if [ "$build" = 1 ]; then
  echo "Building Pantheon (release)..."
  (cd "$repo" && cargo build --release --bin pantheon --bin pantheon-cli)
  if [ "$with_3d" = 1 ]; then
    echo "Building Pantheon 3D, its native Tauri window (release)..."
    if ! (cd "$repo/src-tauri" && CARGO_TARGET_DIR="$repo/target-3d" cargo build --release); then
      echo "Pantheon 3D did not build, so it won't get a menu entry." >&2
      echo "It needs the WebKitGTK 4.1 development package; see README.md." >&2
      with_3d=0
    fi
  fi
fi

atlas_bin="$repo/target/release/pantheon"
flight_bin="$repo/target-3d/release/pantheon-3d"
[ -x "$atlas_bin" ] || {
  echo "$atlas_bin is missing. Run this without --no-build." >&2
  exit 1
}

mkdir -p "$apps" "$icons"
install -m 644 "$repo/assets/icon.png" "$icons/pantheon.png"
# Exec paths are quoted, so a checkout path with spaces works
cat >"$apps/pantheon.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Pantheon
GenericName=Atlas of the Universe
Comment=A continuous zoomable map from Earth to the cosmic web
Exec="$atlas_bin"
Path=$repo
Icon=pantheon
Terminal=false
Categories=Science;Astronomy;
Keywords=universe;space;planets;stars;galaxy;atlas;
StartupWMClass=pantheon
DESKTOP

if [ "$with_3d" = 1 ] && [ -x "$flight_bin" ]; then
  install -m 644 "$repo/assets/icon-3d.png" "$icons/pantheon-3d.png"
  cat >"$apps/app.pantheon.atlas3d.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Pantheon 3D
GenericName=Universe Explorer
Comment=Free-flight universe explorer in a native window
Exec="$flight_bin"
Path=$repo
Icon=pantheon-3d
Terminal=false
Categories=Science;Astronomy;
Keywords=universe;space;planets;stars;galaxy;3d;
StartupWMClass=pantheon-3d
DESKTOP
else
  remove_entry "$apps/app.pantheon.atlas3d.desktop"
  remove_icon "$icons/pantheon-3d.png" "$repo/assets/icon-3d.png"
  [ "$with_3d" = 1 ] && echo "Skipping Pantheon 3D: $flight_bin was not found." >&2
fi

refresh_caches
echo "Installed. Look for Pantheon in your application menu."
if [ ! -f "$repo/assets/data/textures/8k_earth_daymap.jpg" ]; then
  echo "For real planet maps and spacecraft tracks, run: $repo/tools/fetch_data.py"
fi
