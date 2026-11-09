#!/usr/bin/env bash
# Start an installed Pantheon 3D (the .deb or .rpm) in its real window under Xvfb, and
# check through the window's test hooks that it runs from the package: the backend finds
# the package's catalogs and fonts, the page boots, and WebKit's data goes to the user's
# data folder rather than beside the binary.
#
#   packaging/check-pantheon-3d.sh OUT_DIR
#
# Needs xvfb-run, curl, python3 and pgrep. Leaves the window's log, the backend's
# /api/meta reply and a screenshot of the window in OUT_DIR. The release workflow runs it.
set -euo pipefail

out="$(mkdir -p "$1" && cd "$1" && pwd)"
port=8090
base="http://127.0.0.1:$port"
host="Host: 127.0.0.1:$port"

fail() {
  echo "FAILED: $*" >&2
  echo "--- window log" >&2
  cat "$out/log.txt" >&2
  exit 1
}

PANTHEON_QA=1 PANTHEON_START_URL='/?qa' PANTHEON_PORT=$port PANTHEON_SHOT_DIR="$out" \
  xvfb-run -a -s '-screen 0 1600x1000x24' pantheon-3d >"$out/log.txt" 2>&1 &
runner=$!
stop() {
  local pid
  for pid in $(pgrep -x pantheon-3d || true); do
    kill "$pid" || true
  done
  wait "$runner" 2>/dev/null || true
}
trap stop EXIT

for _ in $(seq 60); do
  if curl -fsS -H "$host" "$base/api/meta" -o "$out/meta.json" 2>/dev/null; then
    break
  fi
  sleep 2
done
[ -s "$out/meta.json" ] || fail "the backend never answered"
python3 - "$out/meta.json" <<'PY' || fail "the package's catalogs were not found"
import json, sys
m = json.load(open(sys.argv[1]))
print(f"backend: {m['stars']} stars, {m['exo_systems']} exoplanet systems, kernels: {m['kernels']}")
assert m["stars"] > 1000 and m["exo_systems"] > 500
PY

for path in / /js/main.js /fonts/Inter.ttf /data/gaia_nearby.csv /data/ne_110m_land.geojson; do
  code=$(curl -s -o /dev/null -w '%{http_code}' -H "$host" "$base$path")
  echo "GET $path: $code"
  [ "$code" = 200 ] || fail "$path answered $code"
done

for _ in $(seq 90); do
  grep -q '\[pantheon\] ready' "$out/log.txt" && break
  sleep 2
done
grep '\[pantheon\] ready' "$out/log.txt" || fail "the page never finished starting"

ask() {
  curl -fsS -X POST -H "$host" --data "$1" "$base/api/qa/eval?timeout=60"
  echo
}
echo "window: $(ask 'window.__pantheon.state()')"
ask 'window.__pantheon.capture("window")' >/dev/null
for _ in $(seq 30); do
  [ -s "$out/window.jpg" ] && break
  sleep 1
done
[ -s "$out/window.jpg" ] || fail "the window sent no screenshot"
echo "screenshot: $out/window.jpg ($(wc -c <"$out/window.jpg") bytes)"

webview="${XDG_DATA_HOME:-$HOME/.local/share}/pantheon/webview-data"
[ -d "$webview" ] || fail "WebKit's data is not in $webview"
echo "WebKit's data: $webview"
echo "Pantheon 3D ran from its package."
