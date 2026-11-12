#!/usr/bin/env bash
# serve.sh — run the Zenoh sandbox fully LOCAL: no internet, no Docker.
# Starts a local remote-api router + serves ../app as it is in the tree, then
# opens it. Run prep.sh ONCE (with internet) first to fetch the router.
#
# Ports: app on :8080, router WebSocket on :10000 (the app's localhost default).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd "$HERE/.." && pwd)/app"
BRIDGE="$HERE/bin/zenoh-bridge-remote-api"
WEB_PORT="${WEB_PORT:-8080}"
WS_PORT="${WS_PORT:-10000}"

[ -f "$APP_DIR/index.html" ] || { echo "app not found at $APP_DIR"; exit 1; }
[ -x "$BRIDGE" ] || { echo "router binary missing — run ./prep.sh first (needs internet once)"; exit 1; }
command -v python3 >/dev/null || { echo "python3 not found (ships with macOS; install on Linux)"; exit 1; }

pids=()
cleanup() { echo; echo "stopping…"; for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT INT TERM

echo "==> starting local Zenoh router (remote-api) on ws://localhost:${WS_PORT}"
"$BRIDGE" --ws-port "$WS_PORT" >/tmp/zenoh-sandbox-router.log 2>&1 &
pids+=("$!")

echo "==> serving the sandbox at http://localhost:${WEB_PORT}"
python3 "$HERE/static-server.py" "$WEB_PORT" "$APP_DIR" &
pids+=("$!")

sleep 1
url="http://localhost:${WEB_PORT}"
case "$(uname -s)" in Darwin) open "$url" 2>/dev/null || true ;; Linux) xdg-open "$url" 2>/dev/null || true ;; esac

cat <<EOF

  Zenoh sandbox is running locally — no internet required.
    app:    $url
    router: ws://localhost:${WS_PORT}  (the app's default — just click Connect)

  Click Connect, start the 🌡 sensor, Listen on demo/**, and watch it move.
  Router log: /tmp/zenoh-sandbox-router.log
  Press Ctrl-C to stop.
EOF
wait
