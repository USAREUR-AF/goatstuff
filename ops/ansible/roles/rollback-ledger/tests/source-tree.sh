#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# source-tree.sh: the row the role writes names the tree it was rendered from in
# ONE key, `source_tree`, and never again at the top level.
#
# Runs the real role on localhost against a scratch ledger, with no privilege
# escalation, in five cases:
#   1. GOAT_DEPLOY_SOURCE_TREE set (every play through ansible-preflight.sh),
#      assembled marker present: source_tree is the exported object, verbatim.
#   2. no export, assembled marker with goatstuff and pre-disclosure patches:
#      source_tree is built from the marker.
#   3. no export, no marker (a clone outside the wrapper): no source_tree.
#   4. a ledger whose last row is the old shape (top-level goatcore_ref,
#      overlay_commit, embargo): the role reads it as the prior row, appends
#      the new row, the end-of-play promote marks it `deploy`, and the old row
#      is left byte for byte.
#   5. the same ref deployed again: a new row only when the overlay commit or the
#      set of pre-disclosure patches differs from the prior row's; a plain
#      checkout (no source_tree) keeps the ref-only comparison.
# No row carries top-level goatcore_ref, overlay_commit or embargo.
set -euo pipefail
cd "$(dirname "$0")"
if ! command -v ansible-playbook >/dev/null 2>&1; then
  echo "[rollback-ledger/source-tree] SKIP (ansible-playbook not on PATH)"
  exit 0
fi

S="$(mktemp -d)"
trap 'rm -rf "$S"' EXIT
fails=0
ok() { echo "  ok    $1"; }
bad() { echo "  FAIL  $1"; fails=$((fails + 1)); }

CORE=1111111111111111111111111111111111111111
STUFF=2222222222222222222222222222222222222222
OVER=3333333333333333333333333333333333333333
printf 'goatcore_ref=%s\ngoatstuff_ref=%s\noverlay_commit=%s\noverlay_path=/o\nembargo=B/core.patch:aa,A/core.patch:bb,A/stuff.patch:cc\n' \
  "$CORE" "$STUFF" "$OVER" > "$S/goat-assembled"

# run <case> <ref> [marker]: one play of the role; the ledger is $S/<case>/deploy-history.jsonl.
# $SRC, when set, is exported as GOAT_DEPLOY_SOURCE_TREE; otherwise the play runs without it.
run() {
  local c="$1" ref="$2" mk="${3:-$S/no-marker}" e
  mkdir -p "$S/$c"
  if [ -n "${SRC:-}" ]; then e=(env GOAT_DEPLOY_SOURCE_TREE="$SRC"); else e=(env -u GOAT_DEPLOY_SOURCE_TREE); fi
  "${e[@]}" ANSIBLE_LOCALHOST_WARNING=False ANSIBLE_INVENTORY_UNPARSED_WARNING=False \
    ansible-playbook -i 'localhost,' -c local playbook.yml \
      -e ansible_become=false -e 'ansible_python_interpreter={{ ansible_playbook_python }}'  \
      -e rollback_ref="$ref" -e rollback_assembled_marker="$mk" \
      -e rollback_ledger_dir="$S/$c" \
      -e rollback_ledger_owner="$(id -un)" -e rollback_ledger_group="$(id -gn)" \
      > "$S/$c.log" 2>&1 || { tail -40 "$S/$c.log"; return 1; }
}

# last <case> <python expression over the last row r>: prints the value.
last() {
  python3 -c 'import json,sys; r=json.loads(open(sys.argv[1]).read().splitlines()[-1]); print(json.dumps(eval(sys.argv[2]), sort_keys=True, separators=(",", ":")))' \
    "$S/$1/deploy-history.jsonl" "$2"
}

no_dupes() {
  local got
  got="$(last "$1" '[k for k in ("goatcore_ref", "overlay_commit", "embargo") if k in r]')"
  if [ "$got" = "[]" ]; then ok "$1: no top-level goatcore_ref, overlay_commit or embargo"; else bad "$1: top-level keys $got"; fi
}

echo "[rollback-ledger/source-tree] case 1: exported source tree wins"
EXPORTED='{"embargo":["A"],"goatcore":"'$CORE'","goatcore_ref":"'$CORE'","goatstuff":"'$STUFF'","overlay":"'$OVER'"}'
SRC="$EXPORTED" run exported ref-1 "$S/goat-assembled"
got="$(last exported 'r.get("source_tree")')"
if [ "$got" = "$EXPORTED" ]; then ok "exported: source_tree is the exported object"; else bad "exported: source_tree $got"; fi
no_dupes exported

echo "[rollback-ledger/source-tree] case 2: marker only"
SRC="" run marker ref-1 "$S/goat-assembled"
want='{"embargo":["A","B"],"goatcore":"'$CORE'","goatstuff":"'$STUFF'","overlay":"'$OVER'"}'
got="$(last marker 'r.get("source_tree")')"
if [ "$got" = "$want" ]; then ok "marker: source_tree built from the marker"; else bad "marker: source_tree $got, want $want"; fi
no_dupes marker

echo "[rollback-ledger/source-tree] case 3: plain checkout outside the wrapper"
SRC="" run clone ref-1
got="$(last clone '"source_tree" in r')"
if [ "$got" = "false" ]; then ok "clone: no source_tree"; else bad "clone: source_tree present"; fi
no_dupes clone

echo "[rollback-ledger/source-tree] case 4: an old-shape row before it"
mkdir -p "$S/old"
OLD='{"action": "deploy", "artifact_type": "image", "component": "fixture-component", "embargo": ["A"], "goatcore_ref": "'$CORE'", "image": "ref-0", "operator": "x@localhost", "overlay_commit": "'$OVER'", "prior_ref": "", "ref": "ref-0", "restore_pointer": "", "reversible": true, "source_tree": {"goatcore": "'$CORE'", "overlay": "'$OVER'"}, "ts": "2026-11-01T00:00:00Z", "via": "ansible-role-rollback-ledger"}'
printf '%s\n' "$OLD" > "$S/old/deploy-history.jsonl"
SRC="$EXPORTED" run old ref-1 "$S/goat-assembled"
rows="$(wc -l < "$S/old/deploy-history.jsonl" | tr -d ' ')"
if [ "$rows" = 2 ]; then ok "old: one row appended"; else bad "old: $rows rows"; fi
if [ "$(head -1 "$S/old/deploy-history.jsonl")" = "$OLD" ]; then ok "old: the old row is unchanged"; else bad "old: the old row changed"; fi
got="$(last old '[r.get("prior_ref"), r.get("action")]')"
if [ "$got" = '["ref-0","deploy"]' ]; then ok "old: prior_ref read from the old row; new row promoted"; else bad "old: $got"; fi
no_dupes old

echo "[rollback-ledger/source-tree] case 5: same ref, the tree changes"
rows_in() { wc -l < "$S/$1/deploy-history.jsonl" | tr -d ' '; }
TREE_A='{"goatcore":"'$CORE'","overlay":"'$OVER'"}'
TREE_P='{"embargo":["A"],"goatcore":"'$CORE'","overlay":"'$OVER'"}'
OVER2=4444444444444444444444444444444444444444
TREE_O='{"embargo":["A"],"goatcore":"'$CORE'","overlay":"'$OVER2'"}'
SRC="$TREE_A" run tree ref-1
SRC="$TREE_A" run tree ref-1
if [ "$(rows_in tree)" = 1 ]; then ok "tree: same ref, same tree: no new row"; else bad "tree: $(rows_in tree) rows after an unchanged re-run"; fi
SRC="$TREE_P" run tree ref-1
got="$(last tree '[r.get("ref"), r.get("prior_ref"), r.get("action"), r["source_tree"].get("embargo")]')"
if [ "$(rows_in tree)" = 2 ] && [ "$got" = '["ref-1","ref-1","deploy",["A"]]' ]; then ok "tree: a patch added with the same ref writes a promoted row"; else bad "tree: $(rows_in tree) rows, last $got"; fi
SRC="$TREE_P" run tree ref-1
if [ "$(rows_in tree)" = 2 ]; then ok "tree: the same patched tree again: no new row"; else bad "tree: $(rows_in tree) rows after re-running the patched tree"; fi
SRC="$TREE_O" run tree ref-1
if [ "$(rows_in tree)" = 3 ]; then ok "tree: a new overlay commit with the same ref writes a row"; else bad "tree: $(rows_in tree) rows after an overlay change"; fi
SRC="$TREE_A" run tree ref-1
got="$(last tree '"embargo" in r["source_tree"]')"
if [ "$(rows_in tree)" = 4 ] && [ "$got" = false ]; then ok "tree: the patch removed writes a row without it"; else bad "tree: $(rows_in tree) rows, embargo still present: $got"; fi
SRC="" run tree ref-1
if [ "$(rows_in tree)" = 4 ]; then ok "tree: a plain checkout with the same ref keeps the ref-only comparison"; else bad "tree: $(rows_in tree) rows after a plain-checkout run"; fi

if [ "$fails" -gt 0 ]; then
  echo "[rollback-ledger/source-tree] FAIL ($fails)"
  exit 1
fi
echo "[rollback-ledger/source-tree] PASS"
