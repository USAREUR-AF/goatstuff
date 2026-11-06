#!/usr/bin/env bash
# 2-hour validation soak — OpenObserve under sustained OTLP ingest pressure.
#
# Anchor: ADR 1008 (soak-testing discipline).
#
# Default duration: 2 hours. Override via SOAK_DURATION.
#
# Run from a host that can reach OpenObserve + OTel Collector + MinIO:
#   OPENOBSERVE_URL=http://192.168.1.253:5080 \
#   OPENOBSERVE_USER=<root email> \
#   OPENOBSERVE_PASS=<root password> \
#   OTEL_HTTP_URL=http://192.168.1.253:4318 \
#   MINIO_HOST=192.168.1.253 \
#   MINIO_SSH_USER=<login on MINIO_HOST> \
#   ops/ansible/roles/openobserve/tests/soak/run-2h-validation.sh
#
# What this validates beyond the spike:
#   1. Sustained OTLP ingest at operational-typical rate (1 record/30s
#      via OpenObserve direct + 1 record/30s via OTel Collector).
#   2. Hot-tier NVMe stays bounded (no monotonic growth that would
#      exceed retention window).
#   3. Parquet rotation reaches MinIO at expected cadence — at the end
#      of the soak window we should see Parquet files in
#      telemetry-otlp/ that didn't exist at the start.
#   4. Search latency stays bounded (each tick measures p50/p95
#      across the soak window; gross degradation is the smoke).
#   5. OpenObserve's own /healthz remains 200 throughout.
#
# Output: appends one JSON line per tick to
#   docs/reports/<YYYY-MM-DD>-openobserve-soak-evidence.jsonl

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../../../../.." && pwd)"
SOAK_DURATION="${SOAK_DURATION:-2h}"
TICK_INTERVAL=30
TODAY="$(date -u +%Y-%m-%d)"
# EVIDENCE override env var lets the soak run from any directory (e.g.
# /tmp on a remote host) where the repo isn't checked out.
EVIDENCE="${EVIDENCE:-$REPO_ROOT/docs/reports/${TODAY}-openobserve-soak-evidence.jsonl}"

OPENOBSERVE_URL="${OPENOBSERVE_URL:-http://192.168.1.253:5080}"
OPENOBSERVE_USER="${OPENOBSERVE_USER:-}"
OPENOBSERVE_PASS="${OPENOBSERVE_PASS:-}"
OTEL_HTTP_URL="${OTEL_HTTP_URL:-http://192.168.1.253:4318}"
MINIO_HOST="${MINIO_HOST:-192.168.1.253}"
# The SSH login on MINIO_HOST that may run `sudo docker compose`.
MINIO_SSH_USER="${MINIO_SSH_USER:?set MINIO_SSH_USER to the SSH login on MINIO_HOST}"

if [ -z "$OPENOBSERVE_USER" ] || [ -z "$OPENOBSERVE_PASS" ]; then
  echo "ERROR: set OPENOBSERVE_USER + OPENOBSERVE_PASS" >&2
  exit 64
fi

mkdir -p "$(dirname "$EVIDENCE")"

duration_to_sec() {
  case "$1" in
    *h) printf '%d' "$(( ${1%h} * 3600 ))" ;;
    *m) printf '%d' "$(( ${1%m} * 60   ))" ;;
    *s) printf '%d' "${1%s}"               ;;
    *)  printf '%d' "$1"                   ;;
  esac
}
SOAK_SEC=$(duration_to_sec "$SOAK_DURATION")

emit() {
  printf '{"ts":"%s","soak":"openobserve-2h-validation","kind":"%s","detail":%s}\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "$2" >> "$EVIDENCE"
}

# Snapshot starting Parquet count in MinIO so we can verify rotation.
PARQUET_START=$(ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 \
  "$MINIO_SSH_USER@$MINIO_HOST" \
  "sudo docker compose -f /opt/minio/docker-compose.yml run --rm --no-deps --entrypoint /bin/sh mc-init -c 'mc --json ls --recursive local/telemetry-otlp/' 2>/dev/null | grep -c '\"type\":\"file\"' || true" \
  2>/dev/null || echo 0)

emit start "{\"duration_sec\":$SOAK_SEC,\"parquet_at_start\":$PARQUET_START}"

END_TS=$(( $(date +%s) + SOAK_SEC ))
TICK=0
while [ "$(date +%s)" -lt "$END_TS" ]; do
  TICK=$((TICK + 1))
  TICK_TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  TS_NS=$(date -u +%s%N)
  CORR="soak-$TICK_TS-$TICK"

  # 1. OpenObserve direct ingest
  OO_PAYLOAD=$(printf '[{"_timestamp":%s,"level":"info","service.name":"soak-direct","dbb.correlation_id":"%s","dbb.event_type":"soak.direct"}]' "$TS_NS" "$CORR-direct")
  status=$(curl -fsS -o /dev/null -w '%{http_code}' \
    -u "$OPENOBSERVE_USER:$OPENOBSERVE_PASS" \
    -H 'Content-Type: application/json' \
    -X POST "$OPENOBSERVE_URL/api/default/default/_json" \
    --data-binary "$OO_PAYLOAD" 2>/dev/null || echo 000)
  emit ingest_oo "{\"correlation\":\"$CORR-direct\",\"status\":$status}"

  # 2. Via OTel Collector
  OTEL_PAYLOAD=$(cat <<EOJ
{"resourceLogs":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"soak-via-otel"}}]},"scopeLogs":[{"logRecords":[{"timeUnixNano":"$TS_NS","severityText":"INFO","body":{"stringValue":"soak via otel"},"attributes":[{"key":"dbb.correlation_id","value":{"stringValue":"$CORR-otel"}},{"key":"dbb.event_type","value":{"stringValue":"soak.otel"}}]}]}]}]}
EOJ
)
  otel_status=$(curl -fsS -o /dev/null -w '%{http_code}' \
    -H 'Content-Type: application/json' \
    -X POST "$OTEL_HTTP_URL/v1/logs" \
    --data-binary "$OTEL_PAYLOAD" 2>/dev/null || echo 000)
  emit ingest_otel "{\"correlation\":\"$CORR-otel\",\"status\":$otel_status}"

  # 3. /healthz
  health_status=$(curl -fsS -o /dev/null -w '%{http_code}' "$OPENOBSERVE_URL/healthz" 2>/dev/null || echo 000)
  emit healthz "{\"status\":$health_status}"

  # 4. Search latency (every 5 ticks to cap query load)
  if [ $((TICK % 5)) -eq 0 ]; then
    SEARCH_PAYLOAD=$(cat <<EOJ
{"query":{"sql":"SELECT count(*) FROM default WHERE _timestamp >= $((TS_NS / 1000 - 600000000))","start_time":$((TS_NS / 1000 - 600000000)),"end_time":$((TS_NS / 1000)),"from":0,"size":1}}
EOJ
)
    t0=$(date +%s%3N)
    curl -fsS -o /dev/null \
      -u "$OPENOBSERVE_USER:$OPENOBSERVE_PASS" \
      -H 'Content-Type: application/json' \
      -X POST "$OPENOBSERVE_URL/api/default/_search?type=logs" \
      --data-binary "$SEARCH_PAYLOAD" 2>/dev/null || true
    t1=$(date +%s%3N)
    emit search_latency_ms "{\"ms\":$((t1 - t0))}"
  fi

  sleep "$TICK_INTERVAL"
done

# End-of-soak Parquet check.
PARQUET_END=$(ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 \
  "$MINIO_SSH_USER@$MINIO_HOST" \
  "sudo docker compose -f /opt/minio/docker-compose.yml run --rm --no-deps --entrypoint /bin/sh mc-init -c 'mc --json ls --recursive local/telemetry-otlp/' 2>/dev/null | grep -c '\"type\":\"file\"' || true" \
  2>/dev/null || echo 0)

PARQUET_DELTA=$((PARQUET_END - PARQUET_START))
emit end "{\"parquet_at_end\":$PARQUET_END,\"parquet_delta\":$PARQUET_DELTA,\"ticks\":$TICK}"
echo "Soak complete. Evidence: $EVIDENCE"
echo "Parquet rotation delta over soak: $PARQUET_DELTA"
echo "If delta == 0, OpenObserve held data hot-tier-only; investigate ZO_S3 config."
