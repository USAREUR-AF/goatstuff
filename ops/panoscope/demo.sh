#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Convenience wrapper for the local panoscope demo. Thin layer over
# `docker compose -f docker-compose.dev.yml` with a preflight check and the
# right URL printed for you. Full guide: README-local-demo.md.
#
#   ./demo.sh up        # build (first run) + start; prints the URL
#   ./demo.sh open      # up, then open the browser
#   ./demo.sh status    # container status
#   ./demo.sh logs      # follow all logs (or: ./demo.sh logs synth-fleet)
#   ./demo.sh rebuild    # force a clean image rebuild
#   ./demo.sh down      # stop + remove containers
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE_FILE="${HERE}/docker-compose.dev.yml"
URL="http://127.0.0.1:8080"
DC=(docker compose -f "${COMPOSE_FILE}")

preflight() {
  command -v docker >/dev/null 2>&1 || {
    echo "error: docker not found. Install Docker Desktop: https://docs.docker.com/desktop/" >&2
    exit 1
  }
  docker info >/dev/null 2>&1 || {
    echo "error: Docker daemon not reachable. Start Docker Desktop and retry." >&2
    exit 1
  }
  if command -v lsof >/dev/null 2>&1 && lsof -iTCP:8080 -sTCP:LISTEN >/dev/null 2>&1; then
    echo "warning: TCP 8080 is already in use; panoscope may fail to bind." >&2
  fi
}

cmd="${1:-up}"
case "${cmd}" in
  up)
    preflight
    echo "Building + starting (first build is ~15-20 min cold; cached after)…"
    "${DC[@]}" up --build -d
    echo
    echo "panoscope demo is up → ${URL}"
    echo "Give the fleet ~30s to populate (slow 0.1 Hz peers + 30s schema-decls)."
    echo "Status: ./demo.sh status   Logs: ./demo.sh logs   Stop: ./demo.sh down"
    ;;
  open)
    preflight
    "${DC[@]}" up --build -d
    sleep 3
    ( command -v open >/dev/null 2>&1 && open "${URL}" ) || \
    ( command -v xdg-open >/dev/null 2>&1 && xdg-open "${URL}" ) || \
      echo "Open ${URL} in your browser."
    ;;
  status)
    "${DC[@]}" ps
    ;;
  logs)
    shift || true
    "${DC[@]}" logs -f "$@"
    ;;
  rebuild)
    preflight
    "${DC[@]}" build --no-cache
    ;;
  down)
    "${DC[@]}" down
    ;;
  *)
    echo "usage: $0 {up|open|status|logs [service]|rebuild|down}" >&2
    exit 2
    ;;
esac
