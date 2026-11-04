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
# after moving it. Uninstalling leaves the checkout and its downloads untouched.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
apps="$HOME/.local/share/applications"
icons="$HOME/.local/share/icons/hicolor/256x256/apps"

build=1
with_3d=1
uninstall=0
for arg in "$@"; do
  case "$arg" in
    --no-build) build=0 ;;
    --no-3d) with_3d=0 ;;
    --uninstall) uninstall=1 ;;
    -h | --help)
      sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown option: $arg (try --help)" >&2
      exit 2
      ;;
  esac
done

refresh_caches() {
  command -v update-desktop-database >/dev/null && update-desktop-database "$apps" || true
  command -v gtk-update-icon-cache >/dev/null &&
    gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
}

if [ "$uninstall" = 1 ]; then
  rm -f "$apps/pantheon.desktop" "$apps/pantheon-3d.desktop"
  rm -f "$icons/pantheon.png" "$icons/pantheon-3d.png"
  refresh_caches
  echo "Removed the Pantheon menu entries and icons. Your checkout is untouched."
  exit 0
fi

command -v cargo >/dev/null || {
  echo "cargo was not found. Install Rust from https://rustup.rs, then run this again." >&2
  exit 1
}

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
Categories=Education;Science;Astronomy;
Keywords=universe;space;planets;stars;galaxy;atlas;
StartupWMClass=pantheon
DESKTOP

if [ "$with_3d" = 1 ] && [ -x "$flight_bin" ]; then
  install -m 644 "$repo/assets/icon-3d.png" "$icons/pantheon-3d.png"
  cat >"$apps/pantheon-3d.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Pantheon 3D
GenericName=Universe Explorer
Comment=Free-flight universe explorer in a native window
Exec="$flight_bin"
Path=$repo
Icon=pantheon-3d
Terminal=false
Categories=Education;Science;Astronomy;
Keywords=universe;space;planets;stars;galaxy;3d;
DESKTOP
else
  rm -f "$apps/pantheon-3d.desktop" "$icons/pantheon-3d.png"
  [ "$with_3d" = 1 ] && echo "Skipping Pantheon 3D: $flight_bin was not found." >&2
fi

refresh_caches
echo "Installed. Look for Pantheon in your application menu."
if [ ! -f "$repo/assets/data/textures/8k_earth_daymap.jpg" ]; then
  echo "For real planet maps and spacecraft tracks, run: $repo/tools/fetch_data.py"
fi
