#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# bootstrap-zagros-synth-decode.sh — make a synthetic fleet decodable
# in panoscope's "Decoded" view.
#
# WHY THIS EXISTS (the bug it fixes)
# ----------------------------------
# On a slot-pool goatnet, every synthetic peer publishes under its OWN per-peer UUID slot
# anchor (the ADR 1025 uuid-anchor model), e.g.
#     40655337fd477237eb962df35fd7b7a4/tarsier/alpha-fob/contacts/v1
# panoscope's decoder splits the FIRST slash segment as the portal vendor,
# lists that vendor's topics, and matches `key_pattern` exactly to find the
# topic's declared `format: protobuf:<Msg>` + its uploaded schema
# (crates/panoscope-core/src/portal.rs::find_topic_by_key + fetch_fds).
#
# Run AFTER the slot-pool synth-vendors deploy play has onboarded
# the fleet (each peer gets a portal vendor + alias + slot). This registers
# each live topic + uploads its schema so the Decoded view renders cleanly
# (status=decoded) — the synth-vendor binary already emits protobuf on
# the wire, so the only missing piece is the portal-side schema metadata.
#
# SCHEMA UPLOAD = .proto SOURCE (no protoc needed)
# ------------------------------------------------
# The portal compiles `.proto` source to a FileDescriptorSet in-process when
# you upload with `format=protobuf-source` (internal/protocompile, closes
# F-158). So this script uploads the raw `.proto` files directly — no local
# protoc, no .fds build step. The notional protos are single-file with no
# imports, which is exactly the single-file-source case the portal supports.
#
# WHAT IT DOES
# ------------
#   1. Discovers the live `<anchor>/<animal>/<site>/<thing>/v1` synth topics
#      from the panoscope snapshot (the authoritative "what's actually
#      publishing" source).
#   2. Maps each <animal> → personality .proto + message name via the
#      sibling notional-topic-manifest.tsv (keyed on the animal = segment 2).
#   3. POSTs the topic under vendor=<anchor> with `format=protobuf:<Msg>`
#      (idempotent — 409 swallowed), then uploads the personality `.proto`
#      with `format=protobuf-source` (portal compiles + persists the FDS).
#
# Idempotent: re-run any time. Topic POST 409 = already present; schema POST
# 200 on identical compiled body. Additive API calls only — never a
# router/ACL redeploy.
#
# Requires:
#   - jq, curl  (NO protoc — the portal compiles the .proto)
#   - PORTAL_URL      (has a default below)
#   - PANOSCOPE_URL   (has a default below)
#   - PORTAL_OPERATOR_API_KEY  (operator super-user key; from .envrc.local.d)

set -euo pipefail

PORTAL_URL="${PORTAL_URL:-https://portal.zagros.netbird.zagros.example}"
PANOSCOPE_URL="${PANOSCOPE_URL:-https://panoscope.zagros.netbird.zagros.example}"
: "${PORTAL_OPERATOR_API_KEY:?set PORTAL_OPERATOR_API_KEY}"

cd "$(dirname "$0")"
PROTO_DIR="proto-notional"
MANIFEST="notional-topic-manifest.tsv"
AUTH="Authorization: Bearer ${PORTAL_OPERATOR_API_KEY}"

# animal → "<proto_file>" + "<message_name>"  (keyed on path segment 2 of the
# notional/<animal>/... rows in the manifest). One row per animal is enough;
# all of an animal's topics share a personality.
echo "== building animal -> personality map from ${MANIFEST} =="
# Pairwise parallel arrays + a linear lookup instead of associative
# arrays: operator-run on macOS /bin/bash 3.2 (#227). N is a handful
# of animals.
ANIMALS=()
ANIMAL_PROTOS=()
ANIMAL_MSGS=()
animal_index_for() {
  local i=0
  while [ "$i" -lt "${#ANIMALS[@]}" ]; do
    if [ "${ANIMALS[$i]}" = "$1" ]; then
      printf '%s' "$i"
      return 0
    fi
    i=$((i + 1))
  done
  return 1
}
while IFS=$'\t' read -r key proto_rel msgname _fmt; do
  case "$key" in '#'*|'') continue ;; esac
  animal="$(echo "$key" | cut -d/ -f2)"
  # A repeated animal OVERWRITES its earlier row (the manifest carries
  # one duplicate today) — the same last-row-wins the associative array
  # had, and the count stays the number of DISTINCT animals.
  dup_i="$(animal_index_for "$animal" || true)"
  if [ -n "$dup_i" ]; then
    ANIMAL_PROTOS[$dup_i]="$(basename "$proto_rel")"
    ANIMAL_MSGS[$dup_i]="${msgname##*.}"
    continue
  fi
  ANIMALS+=("$animal")
  ANIMAL_PROTOS+=("$(basename "$proto_rel")")
  # SHORT message name (e.g. Entity), not the fully-qualified
  # notional.tracks_demo.v1.Entity. The portal's protobuf-source compiler
  # (internal/protocompile, single-file) emits the message WITHOUT its
  # package, so the persisted FDS's top-level name is the bare `Entity`.
  # The topic's `format: protobuf:<Name>` must match that bare name or the
  # upload is rejected 422 format_mismatch (and the decoder couldn't resolve
  # it either). This differs from the protoc `--include_imports` .fds path
  # , which preserves the package — see F-189 note. panoscope's decoder
  # resolves either (full-name lookup OR short-name fallback), so the bare
  # name decodes correctly.
  ANIMAL_MSGS+=("${msgname##*.}")
done < "$MANIFEST"
echo "  ${#ANIMALS[@]} animals mapped"

echo
echo "== discovering live synth topics from panoscope =="
# while-read, not mapfile: operator-run on macOS /bin/bash 3.2 (#227).
TOPICS=()
while IFS= read -r t || [ -n "$t" ]; do
  [ -n "$t" ] && TOPICS+=("$t")
done < <(curl -sk --max-time 15 "${PANOSCOPE_URL}/api/snapshot" \
  | jq -r '[.edges[].topic] + [.nodes[].publishes[]?] | unique
           | map(select(test("^[0-9a-f]{32}/")
                        and (test("/hello/|goat-precheck|/selftest$")|not))) | .[]')
echo "  ${#TOPICS[@]} UUID-slotted synth topics live"

echo
echo "== registering topics + uploading .proto schemas =="
# Zero-topics guard: under set -u, bash < 4.4 aborts expanding an EMPTY
# array, and an empty panoscope feed (down, timeout, nothing live) is a
# realistic path. Length is safe when the loop expansion is not.
if [ "${#TOPICS[@]}" -eq 0 ]; then
  echo "no live synth topics on panoscope — nothing to register"
  exit 0
fi
reg=0; sch=0; skip=0; fail=0
for key in "${TOPICS[@]}"; do
  vendor="$(echo "$key" | cut -d/ -f1)"
  animal="$(echo "$key" | cut -d/ -f2)"
  ai="$(animal_index_for "$animal" || true)"
  proto_file=""
  msgname=""
  if [ -n "$ai" ]; then
    proto_file="${ANIMAL_PROTOS[$ai]}"
    msgname="${ANIMAL_MSGS[$ai]}"
  fi
  if [ -z "$msgname" ]; then
    echo "  ? SKIP $key — animal '$animal' not in manifest"
    skip=$((skip+1)); continue
  fi

  # 1. register the topic (idempotent; 409 = already present)
  body=$(jq -nc --arg k "$key" --arg f "protobuf:${msgname}" \
    --arg d "ZAGROS synthetic ${animal} — auto-registered for panoscope decode" \
    '{key_pattern:$k, format:$f, description:$d}')
  rt=$(mktemp)
  st=$(curl -sk -o "$rt" -w "%{http_code}" -X POST -H "$AUTH" \
    -H "Content-Type: application/json" -d "$body" \
    "${PORTAL_URL}/api/vendors/${vendor}/topics")
  case "$st" in
    2*) reg=$((reg+1)) ;;
    409) : ;;  # already registered
    *)  echo "  ! $key — topic POST status=$st $(cat "$rt")"; fail=$((fail+1)); rm -f "$rt"; continue ;;
  esac
  rm -f "$rt"

  # 2. resolve topic id (by exact key_pattern match)
  tid=$(curl -sk -H "$AUTH" "${PORTAL_URL}/api/vendors/${vendor}/topics" \
    | jq -r --arg k "$key" '.topics[] | select(.key_pattern==$k) | .id' | head -1)
  if [ -z "$tid" ] || [ "$tid" = "null" ]; then
    echo "  ! $key — could not resolve topic id after register"; fail=$((fail+1)); continue
  fi

  # 2b. PATCH the format to the short name (idempotent; corrects topics that
  #     a prior run registered with the fully-qualified name).
  curl -sk -o /dev/null -X PATCH -H "$AUTH" -H "Content-Type: application/json" \
    -d "$(jq -nc --arg f "protobuf:${msgname}" '{format:$f}')" \
    "${PORTAL_URL}/api/vendors/${vendor}/topics/${tid}" || true

  # 3. upload the .proto SOURCE — the portal compiles it to an FDS server-side
  #    (format=protobuf-source). Idempotent: 200 on identical compiled body.
  rt=$(mktemp)
  st=$(curl -sk -o "$rt" -w "%{http_code}" -X POST -H "$AUTH" \
    -F "file=@${PROTO_DIR}/${proto_file};filename=${proto_file}" \
    -F "version=v1" -F "compatibility=none" -F "format=protobuf-source" \
    "${PORTAL_URL}/api/vendors/${vendor}/topics/${tid}/schemas")
  case "$st" in
    2*) echo "  + $key (${msgname})"; sch=$((sch+1)) ;;
    *)  echo "  ! $key — schema POST status=$st $(cat "$rt")"; fail=$((fail+1)) ;;
  esac
  rm -f "$rt"
done

echo
echo "summary: topics_registered=$reg schemas_uploaded=$sch skipped=$skip failed=$fail"
exit $(( fail > 0 ? 1 : 0 ))
