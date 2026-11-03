# Video fabric — operations runbook

Day-to-day procedures for the Block 35 video fabric. §1 substrate
viability is **fully verified on Linux and macOS** (integdev-a +
local Mac, 2026-04-29) — MediaMTX v1.18.0 with the §1.2 yml below
accepts RTSP-TCP ingest at 1280x720@30fps, exposes the active stream
via API with full codec metadata, serves valid HLS master + variant
playlists, and a fetched first segment returns valid MPEG-TS data
(261 KB on integdev, 439 KB on the Mac at the seg-85 mark; both with
0x47 sync bytes confirmed at offsets 0/188/376). The Mac run held
ffmpeg push stable for 13m17s without corruption — well past the
F-019 +10s UDP-corruption window — confirming `-rtsp_transport tcp`
holds across Docker Desktop's UDP-NAT path. **Spike A degraded is
also verified** (cross-host RTSP-TCP push integdev-b → integdev-a
over VPC) — same evidence shape, three HLS segments served. Each
later section is gated on the corresponding spike returning a
verdict; sections marked _planned_ should be updated by the first
runner with what actually happened, not deleted.

**Findings produced during the 2026-04-29 session (substrate verified):**

- F-018 — MediaMTX v1.18.0 requires explicit auth users + explicit path config; defaults reject anonymous publish/read/API. The §1.2 yml below incorporates the working config.
- F-019 — ffmpeg default UDP-transport RTSP push emits 1460-byte RTP packets; MediaMTX defaults to 1440 MTU. UDP silently corrupts at +10s; `-rtsp_transport tcp` triggers MediaMTX remux and the session survives indefinitely. The §1.3 push command below specifies TCP.

Evidence: `../reports/2026-04-29-video-fabric-substrate-evidence.jsonl`.

**Design:** `../design/video-fabric.md`
**Tracker:** `../project/implementation-plan.md` Block 35
**Validation state:** `../project/validation-state.md` §"Video fabric spikes (Block 35)"
**Roadmap:** `../project/roadmap.md` priority risk call-out 24

---

## Table of contents

- [1. Local proof-of-life — see video in a browser in 5 minutes](#1-local-proof-of-life--see-video-in-a-browser-in-5-minutes)
- [2. What §1 proves and what it does not](#2-what-1-proves-and-what-it-does-not)
- [3. Next step — integdev single-node over WireGuard (Spike A)](#3-next-step--integdev-single-node-over-wireguard-spike-a)
- [4. Then — viewer-scale measurement (Spike B)](#4-then--viewer-scale-measurement-spike-b)
- [5. First-source ingest probe (Spike C)](#5-first-source-ingest-probe-spike-c)
- [6. TAK CoT round-trip (Spike D)](#6-tak-cot-round-trip-spike-d)
- [7. Production v1 — MediaMTX deployment (35E)](#7-production-v1--mediamtx-deployment-35e)

---

## 1. Local proof-of-life — see video in a browser in 5 minutes

Goal: prove that the v1 default substrate (MediaMTX) accepts an RTSP
push and re-serves it as WebRTC, on your Mac, with no WireGuard, no
integdev, no mTLS, no Zenoh, no Ory. If this doesn't work, nothing
downstream of it works either.

### 1.1 Prerequisites

- Docker Desktop running on the Mac
- `ffmpeg` installed (`brew install ffmpeg`)
- A browser

### 1.2 Run MediaMTX

**Important:** v1.18.0 has tighter defaults than older tutorials suggest
(F-018)
— API auth on by default, paths must be explicitly configured. A bare
`docker run` will fail. Use this minimal yml:

```bash
mkdir -p /tmp/mtx
cat >/tmp/mtx/mediamtx.yml <<'EOF'
logLevel: info
api: yes
apiAddress: :9997
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
hlsVariant: mpegts
hlsAlwaysRemux: yes
hlsSegmentCount: 3
hlsSegmentDuration: 1s
paths:
  all_others:
EOF

docker run --rm -d --name mtx-spike \
  -p 8554:8554 \
  -p 8888:8888 \
  -p 8889:8889 \
  -p 8189:8189/udp \
  -p 9997:9997 \
  -v /tmp/mtx/mediamtx.yml:/mediamtx.yml:ro \
  -e MTX_WEBRTCADDITIONALHOSTS=127.0.0.1 \
  bluenviron/mediamtx@sha256:0a232bfb1bea17d547f23b0e83f9aa5e099c558b9c156e414d747eb2888091dc
```

Ports: `8554` RTSP ingest, `8888` HLS read, `8889` WebRTC read,
`8189/udp` WebRTC ICE, `9997` API. `MTX_WEBRTCADDITIONALHOSTS`
publishes an extra ICE candidate so the browser can reach UDP through
Docker Desktop's NAT (macOS only). The `paths: all_others:` catchall
allows RTSP push to any path name; the `authInternalUsers: user: any`
block grants anonymous access for §1 only — production must replace
with JWT-based auth via Block 21 Ory.

The image is pinned to its v1.18.0 digest above (verified-good on
both integdev-a and local Mac 2026-04-29). Do **not** revert to
`:latest` for anything past this proof-of-life — production v1 (§7)
builds from source per ADR
[`0708`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0708-build-zenoh-from-source.md) pattern. The pin is
doubly important because upstream's tightening of defaults (e.g. the
v1.18.0 changes captured as F-018) is exactly the kind of breaking
change that compounds over time.

### 1.3 Push a synthetic stream

In a second terminal:

```bash
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000 \
  -c:v libx264 -preset ultrafast -tune zerolatency \
  -c:a aac -ar 48000 \
  -rtsp_transport tcp \
  -f rtsp rtsp://localhost:8554/test
```

`testsrc` generates the SMPTE color bars + counter; `sine` generates
a 1 kHz tone. `-re` paces realtime. `-tune zerolatency` matches what
a real camera does.

**Important: `-rtsp_transport tcp` is mandatory** — see
F-019. The
default UDP transport silently corrupts the session at +10s due to a
20-byte mismatch between ffmpeg's default RTP packet size (1460) and
MediaMTX's default RTP MTU (1440). With TCP transport, MediaMTX's
remux logic handles the size mismatch; the session survives
indefinitely.

### 1.4 View in a browser

Open `http://localhost:8889/test` — you should see the test pattern
and hear the tone. HLS at `http://localhost:8888/test/index.m3u8`
also works.

### 1.5 Stop

Ctrl-C the ffmpeg process, then Ctrl-C MediaMTX. Container has
`--rm` so nothing to clean up.

### 1.6 If it didn't work

- WebRTC blank but HLS works → ICE is the problem. Verify
  `8189/udp` is mapped and `MTX_WEBRTCADDITIONALHOSTS` is set.
  On macOS, Docker Desktop's UDP forwarding is finicky; HLS is
  the fallback evidence-of-life.
- ffmpeg "method DESCRIBE failed: 404" → the stream path doesn't
  exist yet. MediaMTX auto-creates paths on first publish; verify
  the URL matches between publisher and viewer.
- ffmpeg "Connection refused" → MediaMTX isn't listening yet, or
  port 8554 didn't map.

When you've seen the pattern in the browser, §1 on macOS is green
(verified 2026-04-29 on Docker Desktop 28.0.4 + macOS 26.3 + ffmpeg
8.1; evidence row in
`../reports/2026-04-29-video-fabric-substrate-evidence.jsonl`).

### 1.7 Verifying without a browser (curl-only)

If you're driving §1 from a script (or from a remote SSH session
without browser access), verify substrate viability via the API +
HLS playlist + RTSP-pull instead of the browser. With the §1.2 yml
applied and ffmpeg pushing in another terminal:

```bash
# API confirms stream is active with codec metadata
curl -sS http://127.0.0.1:9997/v3/paths/list

# HLS playlist + variant + first segment (cookie-jar required across
# separate curl invocations — the variant URL carries a session token
# but MediaMTX also requires a session cookie set on the master
# request; -c/-b carries it through, alternative is a single -L chain)
curl -sSL -c /tmp/jar -b /tmp/jar -o /tmp/index.m3u8 \
  http://127.0.0.1:8888/test/index.m3u8
VARIANT=$(grep -v '^#' /tmp/index.m3u8 | grep -v '^[[:space:]]*$' | head -1)
curl -sSL -c /tmp/jar -b /tmp/jar -o /tmp/variant.m3u8 \
  "http://127.0.0.1:8888/test/$VARIANT"
SEG=$(grep -v '^#' /tmp/variant.m3u8 | grep -v '^[[:space:]]*$' | head -1)
curl -sSL -c /tmp/jar -b /tmp/jar -o /tmp/seg0.ts \
  "http://127.0.0.1:8888/test/$SEG"
xxd -p -l 1 /tmp/seg0.ts   # 47 = MPEG-TS sync byte; valid segment

# RTSP pull (records 2s of stream to a file)
ffmpeg -rtsp_transport tcp -i rtsp://127.0.0.1:8554/test \
  -t 2 -c copy -f mpegts /tmp/dump.ts
xxd -p -l 1 /tmp/dump.ts   # 47 = MPEG-TS sync byte
```

`file /tmp/seg0.ts` reports `MPEG transport stream data` on Linux
GNU `file`, but BSD `file` on macOS reports a generic `data` —
checking the sync byte (`xxd -p -l 1` returns `47`) is the
filesystem-tool-portable check. Verify also at offset 188 and 376
to be sure (`xxd -p -s 188 -l 1` etc.) — TS packets are 188 bytes
each, every one starts with 0x47.

Substrate is green when API returns the path with `ready:true` and
`source.type: rtspSession`, AND either HLS playlist is non-empty OR
RTSP pull writes a non-zero MPEG-TS file. Both should work; if only
one does, file a finding noting which path is broken.

**Verified end-to-end on integdev-a + local Mac 2026-04-29**
(evidence:
`../reports/2026-04-29-video-fabric-substrate-evidence.jsonl`):

- API returns `{"name":"...","ready":true,"available":true,"online":true,"source":{"type":"rtspSession",...},"tracks":["H264","MPEG-4 Audio"],"codecProps":{"width":1280,"height":720,"profile":"High 4:4:4 Predictive","level":"3.1"}}`
- HLS master playlist returns 200 with bandwidth 250-420 kbps and codec `avc1.f4001f`
- HLS variant playlist lists multiple `.ts` segments at 8.33s each
- First segment fetch returns 200 with 261-440 KB, sync byte `0x47` at offsets 0 / 188 / 376
- RTSP-TCP pull writes a non-zero MPEG-TS file with sync byte `0x47`
- Mac run held publisher stable for 13m17s with no F-019 corruption

The earlier "HLS 500" / "RTSP-pull 404" symptoms during draft of
this section were both downstream of F-019 —
ffmpeg's UDP transport killed the publisher session at +10s, so
both readers were querying a dead path. Adding `-rtsp_transport tcp`
to the push (now codified in §1.3) fixed both.

---

## 2. What §1 proves and what it does not

| Proves | Does not prove |
| --- | --- |
| MediaMTX accepts RTSP and re-serves WebRTC | WireGuard carries video bitrate without loss |
| The image works at all on this OS / Docker version | MediaMTX scales to 30 viewers × 5 streams (Spike B) |
| One stream + one viewer round-trip succeeds | First-source ingest works end-to-end (Spike C) |
| HLS fallback works | TAK CoT round-trip works (Spike D) |
| | Recording-integrity ledger ingestion works |
| | Zenoh control-plane catalog wire-up works |

The point of §1 is to retire the trivial-failure modes before
spending time on integdev. Cost: 5 minutes, $0.

---

## 3. Next step — integdev single-node over WireGuard (Spike A)

_Planned — runner fills in._

Goal: prove that WireGuard carries the UDP that WebRTC needs at video
bitrate × stream count. See `../design/video-fabric.md`
§"Spike A" for verdict criteria.

Sketch:

1. SSH to integdev-a (`ssh goat@<integdev-a-mesh-ip>` over the
   mesh; see `../project/validation-state.md` for current host IPs).
2. Run MediaMTX on integdev-a — same image as §1, bind to mesh IP
   only, no public exposure. Use `--network=host` on Linux.
3. From integdev-b, push the synthetic stream over WireGuard to
   integdev-a's mesh IP (rather than `localhost`).
4. Open WebRTC viewer in a SOCKS-tunneled browser from your Mac, or
   use a curl-based HLS check from integdev-b: `curl
   http://<integdev-a-mesh>:8888/test/index.m3u8`.
5. Run for 1h with 5 simulated streams + 30 simulated viewers; emit
   per-tick JSONL with packet-loss / jitter / RTT.
6. Verdict: <0.5% packet loss, <50ms p99 jitter, no peer disconnects.

Driver lands at `ops/video-fabric/spikes/A-wg-udp/` once it exists;
evidence at `docs/reports/<date>-video-spike-A-wg-udp.jsonl`.

---

## 4. Then — viewer-scale measurement (Spike B)

_Planned — runner fills in._

Goal: decide D1 — MediaMTX-alone vs MediaMTX+LiveKit. See
`../design/video-fabric.md` §"Spike B"
for verdict criteria. This is the high-information spike — its
verdict decides whether v1 is one container per site or two.

Sketch:

1. Same integdev-a MediaMTX as §3.
2. Synthetic viewer driver (k6 with WebRTC plugin, or a Go program
   built on Pion) ramps viewers 5 → 15 → 30 → 60 against 5 streams.
3. Capture per-tick CPU, mem, per-viewer jitter, drop rate.
4. Repeat against single-node LiveKit (`livekit/livekit-server`).
5. Verdict-per-D1:
   - **MediaMTX holds at 30 × 5** → defer LiveKit; ship MediaMTX-only v1.
   - **MediaMTX falls over** → ship both: MediaMTX for ingest+record,
     LiveKit for viewer fanout.

Driver at `ops/video-fabric/spikes/B-viewer-scale/`; evidence at
`docs/reports/<date>-video-spike-B-viewer-scale.jsonl`.

---

## 5. First-source ingest probe (Spike C)

_Planned — runner fills in once a real upstream feed is reachable
from integdev (or wherever the probe lands)._

Goal: integrate the **first** real video feed end-to-end and produce
a runbook entry that lets the next-source-class integrator follow
the same outline. We do not own the dominant source. We don't yet
know what transport, auth, or discovery the upstream gives us.
Discovery is the work; the runbook entry is the artifact.

Sketch:

1. **Talk to the source operator** (partner POC for a partner feed; vendor for a self-procured camera
   we eventually own). Capture in writing:
   - Transport: RTSP pull / SRT push / multicast / file-drop /
     proprietary / opaque
   - Auth: PSK / cert / unauthenticated within trusted-enclave /
     operator-issued token
   - Discovery: static URL / operator-provided manifest / CoT
     embedded URL / mDNS / something else
   - Bitrate, codec, resolution, framerate
   - Latency budget the operator commits to
   - What we authenticate to / what authenticates to us / where the
     trust boundary actually sits
2. **If the upstream is reachable from integdev**, stand up the
   ingest path on integdev and feed MediaMTX directly. **If not**,
   use an ffmpeg-as-source
   stand-in matching the captured transport + bitrate to validate
   the MediaMTX-side wiring.
3. Verify §1.7's API + HLS / RTSP-pull chain works end-to-end
   against the ingested path.
4. **If the upstream offers an encrypted variant**, configure
   MediaMTX to refuse plaintext fallback on that path; verify a
   plaintext-only push fails.
5. **If we own the source** (eventually), additionally exercise the
   offline-CA / Block 22H credential flow per
   [`enrollment-bundle-runbook.md`](https://github.com/usareur-af/goatcore/blob/main/docs/operations/enrollment-bundle-runbook.md);
   verify cert rotation works without stream interruption. This
   step is **skipped** when we don't own the source.
6. Capture: source operator + transport + auth model + firmware /
   version where applicable + any quirks the integrator had to work
   around.
7. Write the durable runbook entry — structured so the second-source
   integrator can follow it.

Driver at `../../ops/video-fabric/spikes/C-first-source-ingest/`;
evidence at `docs/reports/<date>-video-spike-C-first-source-ingest.jsonl`.

---

## 6. TAK CoT round-trip (Spike D)

_Planned — runner fills in._

Goal: prove TAK CoT-linked video URLs work end-to-end on the mesh
and that federation to a partner fails closed when the release-namespace
path isn't configured. See `../design/video-fabric.md`
§"Spike D" for verdict criteria.

Sketch:

1. Synthetic feed via MediaMTX on integdev or a site host, mesh-only DNS
   (`*.netbird.legacy.example`).
2. Send a CoT message with a video URL pointing at
   `<site>.netbird.legacy.example/cam/<id>` to a mesh-attached ATAK
   client.
3. Verify ATAK plays the video.
4. Federate the CoT via the existing TAK bridge to the partner hub
   (see `bridge-peering.md` — bridge config
   unchanged by this work).
5. Verify federated ATAK either plays the video (if
   `release/<partner>/video/...` is configured — Phase 3+ Block 35F) or
   **fails closed** (if not — proves the boundary works).
6. Capture TAK server version + ATAK client version.

Driver at `ops/video-fabric/spikes/D-tak-cot/`; evidence at
`docs/reports/<date>-video-spike-D-tak-cot.jsonl`.

---

## 7. Production v1 — MediaMTX deployment (35E)

Status as of 2026-04-30:

- Spike A FULL ✅ PASS (30 viewers × 5 streams × 1h over WG)
- Spike B ✅ PASS at 150 viewers × 5 streams — D1 closed: MediaMTX-only
- Spike C 🟡 partial (MediaMTX-side proven; operator handoff gated)
- Spike D 🟡 gated on TAK federation operator coordination
- ADR [`0824`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0824-video-fabric-mediamtx-only-v1.md) Proposed; flips Accepted on the 72h canonical soak passing.

What ships today (committed):

- Ansible role [`ops/ansible/roles/mediamtx/`](../../ops/ansible/roles/mediamtx/) — pinned image digest, F-019 mitigation (`rtspTransports: [tcp]`), JWT auth via Block 21 Ory, smoke-test fixture
- Ansible role [`ops/ansible/roles/feeds-shim/`](../../ops/ansible/roles/feeds-shim/) — systemd unit with hardening, schema-publish bootstrap, depends_on `mediamtx`
- Rust crate [`ops/zenoh-shims/feeds-shim/`](../../ops/zenoh-shims/feeds-shim/) — catalog publisher (cargo check green against zenoh 1.0.4)
- Schemas [`feeds-v1.yaml`](../../ops/zenoh-shims/feeds-shim/schemas/feeds-v1.yaml) + [`feeds-ptz-v1.yaml`](../../ops/zenoh-shims/feeds-shim/schemas/feeds-ptz-v1.yaml) — locked at `attest/schema/feeds/v1` + `attest/schema/feeds-ptz/v1`

What's deferred (depends on Block 17D MinIO substrate landing):

- MinIO `video-archive` bucket Terraform
- Recording-integrity ledger ingestion from MediaMTX hooks
- Canonical-tier 72h soak driver against the production stack with
  ledger ingest + memory-stability check

### 7.1 Running the canonical-tier 72h soak (when ready)

Pre-reqs:

1. Block 17D MinIO substrate deployed at the target site
2. MediaMTX role applied: `ansible-playbook -i inventory/<site>.ini mediamtx.yml`
3. feeds-shim role applied: `ansible-playbook -i inventory/<site>.ini feeds-shim.yml`
4. integdev-b (or another viewer host) has the Pion viewer binary at
   `/tmp/spikeB/whep-viewer` (built from `ops/video-fabric/spikes/B-viewer-scale/`)

Soak driver — re-uses the Spike A full driver with extended hold:

```bash
cd ops/video-fabric/spikes/A-wg-udp
./run.sh --hold 72h --num-viewers 30
```

Verdict criteria from ADR 1008 + ADR 0824 §"Status flip criterion":

- Zero viewer disconnects across 72h
- Zero packet loss (`fraction_lost_max_q8 == 0` on all viewers)
- MediaMTX memory stable (peak RSS doesn't grow more than 2× the
  steady-state baseline observed in the first hour)
- Zero recording-integrity-ledger gaps (when 17D ledger is wired up;
  pending)

Evidence lands at `docs/reports/<date>-video-fabric-v1-soak.jsonl`
(use that filename, not the Spike A path). Update validation-state
and ADR 0824 status flip in the same change.

### 7.2 Day-2 operations (placeholder — fill in as 35E rolls forward)

How to:

- **Integrate a new source-class feed** — follow Spike C's
  discover-at-integration runbook entry template (§5 above), capture
  transport / auth / discovery in writing with the operator, add a
  `mediamtx_paths` entry to the per-site inventory, restart MediaMTX
  via the role's handler, verify via the §1.7 chain.
- **Rotate any credentials the source uses** — cert (if we own the
  source) → re-issue via offline-CA (Block 22H); operator-supplied
  PSK / token → coordinate with the operator. Ansible role's
  `notify: restart mediamtx` picks up new auth on next config render.
- **Investigate a recording-integrity gap** — pending Block 17D
  ledger; will follow the same query pattern as audit gaps.
- **Read the catalog** — query Zenoh for
  `attest/queryable/feeds/<site>/*` from any peer.
- **Issue a PTZ command** — pending feeds-shim subscriber paths
  (forward-looking; schema is locked at `attest/schema/feeds-ptz/v1`).
- **Roll back** — Ansible role's image digest pin makes downgrade
  trivial: change `mediamtx_image_digest` in the per-site `group_vars`
  and re-run the role.
