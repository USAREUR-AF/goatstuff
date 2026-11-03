#!/bin/bash
# MediaMTX role test fixture — Block 35E v1.
#
# Spins up MediaMTX via the role's docker-compose template (no Ansible),
# verifies the §1.7 verification chain (API + HLS master + variant + segment
# sync byte 0x47), tears down. Same shape as Ory's test-fixture.sh.
#
# Usage:
#   ./test-fixture.sh
#
# Pre-reqs: docker, ffmpeg, python3, curl, xxd. (If you ran Spike B's run.sh
# locally, you have all of these.)
#
# This is the fast smoke test. The 72h canonical-tier soak driver per ADR
# 1008 lands separately when 35E ships to vega.
set -euo pipefail

WORK=$(mktemp -d -t mediamtx-test-XXXXXX)
trap 'docker rm -f mediamtx-test 2>/dev/null || true; rm -rf -- "$WORK"' EXIT

# Render templates with minimal vars — mirrors what defaults/main.yml provides.
mkdir -p "$WORK/data" "$WORK/recordings"
cat >"$WORK/mediamtx.yml" <<'EOF'
logLevel: info
api: yes
apiAddress: ":9997"
authMethod: internal
authInternalUsers:
  - user: any
    pass:
    ips: []
    permissions:
      - action: publish
      - action: read
      - action: playback
      - action: api
      - action: metrics
      - action: pprof
rtsp: yes
rtspAddress: ":8554"
rtspTransports: [tcp]
hls: yes
hlsAddress: ":8888"
hlsVariant: mpegts
hlsAlwaysRemux: yes
hlsSegmentCount: 3
hlsSegmentDuration: 1s
webrtc: yes
webrtcAddress: ":8889"
webrtcLocalUDPAddress: ":8189"
webrtcAdditionalHosts: ["127.0.0.1"]
paths:
  all_others:
EOF

echo "=== starting MediaMTX (digest pinned) ==="
docker rm -f mediamtx-test 2>/dev/null || true
docker run --rm -d --name mediamtx-test \
  -p 127.0.0.1:8554:8554 \
  -p 127.0.0.1:8888:8888 \
  -p 127.0.0.1:8889:8889 \
  -p 127.0.0.1:8189:8189/udp \
  -p 127.0.0.1:9997:9997 \
  -v "$WORK/mediamtx.yml":/mediamtx.yml:ro \
  bluenviron/mediamtx@sha256:0a232bfb1bea17d547f23b0e83f9aa5e099c558b9c156e414d747eb2888091dc

# Verify version pin
DIGEST=$(docker inspect mediamtx-test --format '{{index .Image}}')
[[ "$DIGEST" == *"0a232bfb1bea17d547f23b0e83f9aa5e099c558b9c156e414d747eb2888091dc"* ]] || {
  echo "FAIL: digest mismatch: $DIGEST" >&2
  exit 1
}
echo "  digest pin verified: $DIGEST"

sleep 2

# Push synthetic stream
ffmpeg -re -t 30 -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000 \
  -c:v libx264 -preset ultrafast -tune zerolatency \
  -c:a aac -ar 48000 -rtsp_transport tcp \
  -f rtsp rtsp://127.0.0.1:8554/test \
  >"$WORK/ff.log" 2>&1 &
FF_PID=$!
trap 'kill "$FF_PID" 2>/dev/null || true; docker rm -f mediamtx-test 2>/dev/null || true; rm -rf -- "$WORK"' EXIT

sleep 8

# Verify API + HLS chain (§1.7 portable check)
READY=$(curl -sS http://127.0.0.1:9997/v3/paths/list | python3 -c "import sys,json; print(sum(1 for p in json.load(sys.stdin)['items'] if p.get('ready')))")
[[ "$READY" == "1" ]] || { echo "FAIL: $READY paths ready, expected 1" >&2; exit 1; }
echo "  API: 1 path ready"

curl -sSL -c "$WORK/jar" -b "$WORK/jar" -o "$WORK/idx.m3u8" -w "  master HTTP %{http_code}\n" http://127.0.0.1:8888/test/index.m3u8
VARIANT=$(grep -v '^#' "$WORK/idx.m3u8" | grep -v '^[[:space:]]*$' | head -1)
curl -sSL -c "$WORK/jar" -b "$WORK/jar" -o "$WORK/var.m3u8" -w "  variant HTTP %{http_code}\n" "http://127.0.0.1:8888/test/$VARIANT"
SEG=$(grep -v '^#' "$WORK/var.m3u8" | grep -v '^[[:space:]]*$' | head -1)
curl -sSL -c "$WORK/jar" -b "$WORK/jar" -o "$WORK/seg.ts" -w "  segment HTTP %{http_code} bytes=%{size_download}\n" "http://127.0.0.1:8888/test/$SEG"

SYNC=$(xxd -p -l 1 "$WORK/seg.ts")
[[ "$SYNC" == "47" ]] || { echo "FAIL: segment sync byte $SYNC != 47" >&2; exit 1; }
echo "  MPEG-TS sync byte 0x47 confirmed"

echo "=== PASS ==="
