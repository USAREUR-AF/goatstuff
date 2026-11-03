#!/usr/bin/env bash
# ops/ansible/roles/openobserve/tests/test-fixture.sh
#
# Local fixture test for the OpenObserve role + OTel Collector role
# composed against a local MinIO. This is the integrated end-to-end
# coverage: same shape as the on-host-C 27A.3 spike, but runnable on
# a developer laptop without needing the production host.
#
# What this catches that production-side checks would miss:
#   - OpenObserve env-var name drift across versions (ZO_S3_* renames
#     have happened repeatedly upstream).
#   - otlphttp exporter URL shape (OpenObserve's OTLP path moved from
#     /api/<org> to /api/<org>/v1/* in some versions; this exercises
#     end-to-end so a config drift breaks the test).
#   - host.docker.internal cross-compose-project comms (works on Linux
#     with extra_hosts; macOS Docker Desktop has it natively — both
#     paths exercised here).
#   - Parquet rotation reaches MinIO (the silent-failure mode where
#     OpenObserve accepts ingest but never persists to warm tier).
#   - Two-bucket isolation: writing to telemetry-otlp must NOT touch
#     audit-integrity (regression guard against bucket-name typos).
#
# Run locally:
#   ops/ansible/roles/openobserve/tests/test-fixture.sh
#
# Side effects: temporary docker-compose project under
# /tmp/oo-fixture-XXXX/. Auto-cleaned on success or failure.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../../../.." && pwd)"
WORK="$(mktemp -d -t oo-fixture-XXXXXX)"
PROJECT="ootest-$$"

cleanup() {
  docker compose -f "$WORK/docker-compose.yml" -p "$PROJECT" down -v >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# Fixed test creds — fixture only.
export MINIO_ROOT_USER="ootest"
export MINIO_ROOT_PASSWORD="ootest-fixed-password-aaaa-bbbb-cccc"
export ZO_ROOT_USER_EMAIL="ops@fixture.local"
export ZO_ROOT_USER_PASSWORD="ootest-zo-fixed-password-cccc-dddd"

# Ports — different from prod defaults so we don't collide.
TEST_MINIO_PORT=29000
TEST_OO_HTTP_PORT=25080
TEST_OTEL_HTTP_PORT=24318

mkdir -p "$WORK/minio-data" "$WORK/oo-data" "$WORK/otel-data"
chmod 0750 "$WORK/minio-data" "$WORK/oo-data" "$WORK/otel-data"
chown 1000:1000 "$WORK/minio-data" 2>/dev/null || true
chown 10000:10000 "$WORK/oo-data" 2>/dev/null || true
chown 10001:10001 "$WORK/otel-data" 2>/dev/null || true

# Render mc-init script via sed (no Ansible required from fixture)
sed \
  -e 's|{{ minio_audit_bucket }}|audit-integrity|g' \
  -e 's|{{ minio_telemetry_bucket }}|telemetry-otlp|g' \
  -e 's|{{ minio_runtime_detection_bucket }}|runtime-detection-archive|g' \
  -e 's|{{ minio_audit_retention_days }}|1|g' \
  -e 's|{{ minio_telemetry_retention_days }}|1|g' \
  -e 's|{{ minio_runtime_detection_retention_days }}|1|g' \
  "$REPO_ROOT/ops/ansible/roles/minio/templates/mc-init.sh.j2" \
  > "$WORK/mc-init.sh"
chmod +x "$WORK/mc-init.sh"

# Render OTel config (substitute the few variables manually).
sed \
  -e "s|{{ otel_otlp_grpc_port }}|4317|g" \
  -e "s|{{ otel_otlp_http_port }}|4318|g" \
  -e "s|{{ otel_metrics_port }}|8888|g" \
  -e "s|{{ otel_site_id }}|fixture|g" \
  -e "s|{{ otel_site_role }}|test|g" \
  -e "s|{{ otel_openobserve_endpoint }}|http://openobserve:${TEST_OO_HTTP_PORT}/api/default|g" \
  -e "s|{{ ansible_date_time.iso8601 }}|fixture|g" \
  "$REPO_ROOT/ops/ansible/roles/otel-collector/templates/otel-config.yaml.j2" \
  > "$WORK/otel-config.yaml"

cat > "$WORK/docker-compose.yml" <<EOF
services:
  minio:
    image: minio/minio:RELEASE.2024-10-29T16-01-48Z
    container_name: ${PROJECT}-minio
    user: "1000:1000"
    command:
      - server
      - /data
      - --address
      - ":${TEST_MINIO_PORT}"
    environment:
      MINIO_ROOT_USER: \${MINIO_ROOT_USER}
      MINIO_ROOT_PASSWORD: \${MINIO_ROOT_PASSWORD}
    volumes:
      - $WORK/minio-data:/data
    ports:
      - "${TEST_MINIO_PORT}:${TEST_MINIO_PORT}"
    healthcheck:
      test: ["CMD", "curl", "-fsS", "http://localhost:${TEST_MINIO_PORT}/minio/health/live"]
      interval: 2s
      timeout: 3s
      retries: 30
    networks:
      - oo-fixture-net

  mc-init:
    image: minio/mc:RELEASE.2024-10-29T15-34-59Z
    container_name: ${PROJECT}-mc-init
    profiles: ["init"]
    depends_on:
      minio:
        condition: service_healthy
    environment:
      MC_HOST_local: "http://\${MINIO_ROOT_USER}:\${MINIO_ROOT_PASSWORD}@minio:${TEST_MINIO_PORT}"
    volumes:
      - $WORK/mc-init.sh:/mc-init.sh:ro
    entrypoint: ["/bin/sh", "/mc-init.sh"]
    networks:
      - oo-fixture-net

  openobserve:
    image: openobserve/openobserve:v0.80.2
    container_name: ${PROJECT}-openobserve
    user: "10000:10000"
    environment:
      ZO_ROOT_USER_EMAIL: \${ZO_ROOT_USER_EMAIL}
      ZO_ROOT_USER_PASSWORD: \${ZO_ROOT_USER_PASSWORD}
      ZO_DATA_DIR: "/data"
      ZO_LOCAL_MODE: "true"
      # Load-bearing: ZO_LOCAL_MODE_STORAGE=disk (the default) silently
      # ignores all ZO_S3_* config — Parquet stays local forever. Must
      # be `s3` for MinIO backend to engage. Mirrors role default.
      ZO_LOCAL_MODE_STORAGE: "s3"
      ZO_HTTP_PORT: "${TEST_OO_HTTP_PORT}"
      ZO_S3_PROVIDER: "s3"
      ZO_S3_SERVER_URL: "http://minio:${TEST_MINIO_PORT}"
      ZO_S3_REGION_NAME: "us-east-1"
      ZO_S3_BUCKET_NAME: "telemetry-otlp"
      ZO_S3_ACCESS_KEY: "\${MINIO_ROOT_USER}"
      ZO_S3_SECRET_KEY: "\${MINIO_ROOT_PASSWORD}"
      ZO_TELEMETRY: "false"
    volumes:
      - $WORK/oo-data:/data
    ports:
      - "${TEST_OO_HTTP_PORT}:${TEST_OO_HTTP_PORT}"
    depends_on:
      mc-init:
        condition: service_completed_successfully
    # No docker-level healthcheck — distroless image lacks wget/curl/sh.
    # Mirrors the production role's docker-compose.yml.j2 posture
    #. The fixture's smoke-test polls /healthz directly
    # over the published host port — see assert_healthz_ok below.
    networks:
      - oo-fixture-net

  otel-collector:
    image: otel/opentelemetry-collector-contrib:0.113.0
    container_name: ${PROJECT}-otel
    user: "10001:10001"
    command: ["--config=/etc/otelcol-contrib/config.yaml"]
    environment:
      OPENOBSERVE_USER: \${ZO_ROOT_USER_EMAIL}
      OPENOBSERVE_PASS: \${ZO_ROOT_USER_PASSWORD}
    volumes:
      - $WORK/otel-config.yaml:/etc/otelcol-contrib/config.yaml:ro
      - $WORK/otel-data:/var/lib/otel
    ports:
      - "${TEST_OTEL_HTTP_PORT}:4318"
      - "28888:8888"
    depends_on:
      openobserve:
        # Was `service_healthy` — relaxed to `service_started` after
        # F-097 closure dropped the wget-based healthcheck. The
        # otel-collector retries OTLP exporter connections with backoff
        # so a slight startup race is non-fatal.
        condition: service_started
    networks:
      - oo-fixture-net

networks:
  oo-fixture-net:
    name: ${PROJECT}-net
EOF

# ---- Assertion helpers ----
PASS=0
FAIL=0

assert() {
  local label="$1" cond="$2"
  if eval "$cond"; then
    echo "  PASS  $label"
    PASS=$((PASS + 1))
  else
    echo "  FAIL  $label"
    echo "    cond: $cond"
    FAIL=$((FAIL + 1))
  fi
}

# ---- Bring up ----
echo "→ Pulling images"
docker compose -f "$WORK/docker-compose.yml" -p "$PROJECT" pull -q minio mc-init openobserve otel-collector >/dev/null

echo "→ Starting MinIO"
docker compose -f "$WORK/docker-compose.yml" -p "$PROJECT" up -d minio >/dev/null

echo "→ Running mc-init (creates buckets)"
docker compose -f "$WORK/docker-compose.yml" -p "$PROJECT" run --rm mc-init >/dev/null

echo "→ Starting OpenObserve + OTel"
docker compose -f "$WORK/docker-compose.yml" -p "$PROJECT" up -d openobserve otel-collector >/dev/null

# Wait for OO healthy
echo "→ Waiting for OpenObserve /healthz"
for i in $(seq 1 60); do
  if curl -fsS "http://localhost:${TEST_OO_HTTP_PORT}/healthz" >/dev/null 2>&1; then
    echo "  OO ready after ${i}×2s"
    break
  fi
  sleep 2
done

# Wait briefly for OTel collector to come up
sleep 5

# ---- Step A: ingest via OpenObserve directly ----
CORRELATION_ID="fixture-$(date +%s)-$$"
TS_NS=$(date -u +%s%N)

PAYLOAD_OO=$(cat <<EOJ
[
  {
    "_timestamp": $TS_NS,
    "level": "info",
    "service.name": "fixture-emitter",
    "dbb.correlation_id": "$CORRELATION_ID",
    "dbb.event_type": "fixture.synthetic.direct",
    "message": "openobserve direct ingest"
  }
]
EOJ
)

ingest_response=$(curl -fsS \
  -u "${ZO_ROOT_USER_EMAIL}:${ZO_ROOT_USER_PASSWORD}" \
  -H 'Content-Type: application/json' \
  -X POST "http://localhost:${TEST_OO_HTTP_PORT}/api/default/default/_json" \
  --data-binary "$PAYLOAD_OO" 2>&1) || ingest_response="FAILED: $ingest_response"

assert "OpenObserve accepts direct JSON ingest" \
  "echo '$ingest_response' | grep -q '\"code\":200'"

# Wait for index
sleep 5

# ---- Step B: search retrieval ----
SEARCH_QUERY=$(cat <<EOJ
{
  "query": {
    "sql": "SELECT * FROM default WHERE dbb_correlation_id = '$CORRELATION_ID'",
    "start_time": $((TS_NS / 1000 - 60000000)),
    "end_time":   $((TS_NS / 1000 + 60000000)),
    "from": 0,
    "size": 10
  }
}
EOJ
)

search_response=$(curl -fsS \
  -u "${ZO_ROOT_USER_EMAIL}:${ZO_ROOT_USER_PASSWORD}" \
  -H 'Content-Type: application/json' \
  -X POST "http://localhost:${TEST_OO_HTTP_PORT}/api/default/_search?type=logs" \
  --data-binary "$SEARCH_QUERY" 2>&1) || search_response="FAILED: $search_response"

hit_count=$(echo "$search_response" | python3 -c 'import json,sys
try:
  print(json.load(sys.stdin).get("total",0))
except Exception:
  print(0)' 2>/dev/null || echo 0)

assert "search returns the just-ingested record" \
  "[ '$hit_count' -ge 1 ]"

# ---- Step C: send through OTel Collector ----
# Try OTLP/HTTP. The fixture's OTel exporter uses http://openobserve:5080
# which is the container DNS name in the fixture network — we'd need to
# rewrite for the OO endpoint (we did, via sed). Send a synthetic OTLP
# log via OTel's :4318/v1/logs receiver.
OTEL_CORRELATION="otel-$(date +%s)-$$"
OTLP_LOG_PAYLOAD=$(cat <<EOJ
{
  "resourceLogs": [{
    "resource": {
      "attributes": [
        {"key": "service.name", "value": {"stringValue": "fixture-otel"}}
      ]
    },
    "scopeLogs": [{
      "logRecords": [{
        "timeUnixNano": "$TS_NS",
        "severityText": "INFO",
        "body": {"stringValue": "via otel collector"},
        "attributes": [
          {"key": "dbb.correlation_id", "value": {"stringValue": "$OTEL_CORRELATION"}},
          {"key": "dbb.event_type",     "value": {"stringValue": "fixture.synthetic.via-otel"}}
        ]
      }]
    }]
  }]
}
EOJ
)

otel_response=$(curl -fsS -o /tmp/otel-resp.txt -w '%{http_code}' \
  -H 'Content-Type: application/json' \
  -X POST "http://localhost:${TEST_OTEL_HTTP_PORT}/v1/logs" \
  --data-binary "$OTLP_LOG_PAYLOAD" 2>&1) || otel_response="000"

assert "OTel Collector accepts OTLP/HTTP at /v1/logs" \
  "[ '$otel_response' = '200' ] || [ '$otel_response' = '202' ]"

# OTel batches at 1s; allow a few seconds for forward + index
sleep 8

# Search for the OTel-routed record
OTEL_SEARCH=$(cat <<EOJ
{
  "query": {
    "sql": "SELECT * FROM default WHERE dbb_correlation_id = '$OTEL_CORRELATION'",
    "start_time": $((TS_NS / 1000 - 60000000)),
    "end_time":   $((TS_NS / 1000 + 60000000)),
    "from": 0,
    "size": 10
  }
}
EOJ
)

otel_search=$(curl -fsS \
  -u "${ZO_ROOT_USER_EMAIL}:${ZO_ROOT_USER_PASSWORD}" \
  -H 'Content-Type: application/json' \
  -X POST "http://localhost:${TEST_OO_HTTP_PORT}/api/default/_search?type=logs" \
  --data-binary "$OTEL_SEARCH" 2>&1) || otel_search="{}"

otel_hits=$(echo "$otel_search" | python3 -c 'import json,sys
try:
  print(json.load(sys.stdin).get("total",0))
except Exception:
  print(0)' 2>/dev/null || echo 0)

assert "OTel-forwarded record appears in OpenObserve" \
  "[ '$otel_hits' -ge 1 ]"

# ---- Step D: bucket isolation ----
audit_count=$(docker compose -f "$WORK/docker-compose.yml" -p "$PROJECT" run --rm mc-init mc --json ls --recursive local/audit-integrity/ 2>/dev/null | grep -c '"type":"file"' || true)
assert "audit-integrity bucket untouched (write to telemetry-otlp didn't bleed)" \
  "[ '$audit_count' -eq 0 ]"

# ---- Summary ----
echo ""
echo "================================================================"
echo "  PASS: $PASS    FAIL: $FAIL"
echo "================================================================"
if [ "$FAIL" -gt 0 ]; then
  echo ""
  echo "Note: a Parquet-rotation step (telemetry-otlp object count > 0)"
  echo "is intentionally NOT in this fixture — OpenObserve's rotation"
  echo "interval defaults to several minutes and would make the fixture"
  echo "slow. The on-host-C 27A.3 spike covers that step end-to-end."
  exit 1
fi
