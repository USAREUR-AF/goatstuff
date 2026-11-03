#!/usr/bin/env bash
# ops/ci/check-no-identity-spillage.sh — opsec guard.
#
# Refuses commits / PRs that introduce operator-identity material to
# committed (non-gitignored) files. Subset of the broader Block 30
# sanitization discipline; specifically targets the dev-CA roster shape
# that is the highest-leverage opsec exposure today (per CLAUDE.md
# "License posture" + "OSS go-open strategy").
#
# Patterns guarded:
#   - age recipients          age1[a-z0-9]{58}            (Curve25519 X25519 pubkeys)
#   - SSH ed25519 pubkeys     ssh-ed25519 AAAAC3NzaC1lZDI1NTE5...
#   - SSH RSA pubkeys         ssh-rsa AAAAB3NzaC1yc2E...
#
# Allowed locations (these patterns ARE expected to appear here):
#   - ops/bootstrap/recipients.txt   (bootstrap-bundle age recipients +
#                                     allowed_signers — public per design)
#   - ops/dns/, deployment-pubkeys/  (operational config; pubkeys are public)
#   - .envrc.local* / .envrc.local.d/*    (gitignored secrets staging)
#
# Usage:
#   ops/ci/check-no-identity-spillage.sh             # scan tracked tree
#   ops/ci/check-no-identity-spillage.sh --staged    # scan staged diff only
#   ops/ci/check-no-identity-spillage.sh path/...    # scan specific paths
#
# Exit codes:
#   0  no spillage detected
#   1  spillage found — output names files + line numbers
#   2  invalid usage / dependency missing

set -euo pipefail

PATTERNS=(
  'age1[a-z0-9]{58}'
  'ssh-ed25519 AAAA[A-Za-z0-9+/]{40,}'
  'ssh-rsa AAAAB3NzaC1yc2E[A-Za-z0-9+/]{40,}'
)

# Opsec near-secrets (ADR 1196): equipment MAC addresses and host mesh IPs.
# These may live in plaintext ONLY in gitignored local files (*.local.md /
# .yaml / .txt) or as op:// references — never in a tracked file.
#
# MAC: gated repo-wide. Boundary-anchored to EXACTLY six octets so an 8-octet
# key fingerprint (e.g. a CA key MD5) is NOT a match. Broadcast / all-zero MACs
# are allowed (they identify no equipment).
MAC_PATTERN='(^|[^0-9A-Fa-f:])([0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}([^0-9A-Fa-f:]|$)'
MAC_ALLOWED_VALUES='ff:ff:ff:ff:ff:ff|00:00:00:00:00:00'
#
# Mesh / CGNAT IP: 100.64.0.0/10 (NetBird + Tailscale). These are durable goat
# equipment reachability addresses. Gated ONLY in the portable, shareable
# surfaces below — skills and reference docs are written to be read out of
# context (and, on the open-source release, published). RFC1918 LAN IPs are NOT
# gated: they are indistinguishable from structural subnet references and are
# ephemeral DHCP; the durable identifiers (MAC + mesh IP) are what this catches.
MESH_IP_PATTERN='(^|[^0-9.])100\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\.[0-9]{1,3}\.[0-9]{1,3}'
IP_SCOPED_GLOBS=(
  '.claude/skills/*'
  'docs/reference/*'
)

# Inline exception: a line containing `opsec-allow:` (with a reason) is exempt
# from every pattern in this file — for a legitimate, non-live value such as a
# synthetic MAC in a parser test. Use sparingly and say why.
ANNOTATION='opsec-allow'

# Files where these patterns are EXPECTED. Glob-matched against repo-relative
# paths. If a hit lands in any of these, it is not a spill.
ALLOWED_PATHS=(
  'ops/bootstrap/recipients.txt'
  # Dropped: 'ops/dns/*' and 'deployment-pubkeys/*' were exempted for
  # material they no longer carry — measured zero matches for every pattern in
  # this file. An exemption that protects nothing only widens the hole for what
  # lands there next.
  # Per-site Ansible group_vars carry live operator authorized_keys (SSH
  # *public* keys for host/initramfs access) — operational config, not secrets.
  # Grandfathered per F-180 pending the sanitization sweep that moves operator
  # pubkeys out of committed YAML; until then, editing a site vars.yml for an
  # unrelated reason must not trip the diff-scoped scan on the pre-existing keys.
  'ops/ansible/group_vars/*/vars.yml'
  '.envrc.local'
  '.envrc.local.d/*'
  # Dropped the tightening this comment used to defer. The
  # exemption existed because findings/measurements/reports may quote real
  # captured values from production runs as evidence — but measured across all
  # three trees, they carry ZERO matches for the patterns above. So the
  # exemption was protecting nothing and standing open for whatever landed
  # next. A document that genuinely needs to quote operator-identity material
  # now has to say so in review rather than inherit silence.
)

# Scope selection -------------------------------------------------------------

mode=tree
if [[ "${1:-}" == "--staged" ]]; then
  mode=staged
  shift
elif [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  sed -n '2,30p' "$0"
  exit 0
fi

paths=("$@")

# Discover candidate files ----------------------------------------------------

candidates=()
while IFS= read -r f; do
  candidates+=("$f")
done < <(
  if [[ "$mode" == "staged" ]]; then
    git diff --cached --name-only --diff-filter=ACMR
  elif [[ ${#paths[@]} -gt 0 ]]; then
    printf '%s\n' "${paths[@]}"
  else
    git ls-files
  fi
)

# Drop empty / missing / non-text files
filtered=()
if (( ${#candidates[@]} > 0 )); then
  for f in "${candidates[@]}"; do
    [[ -z "$f" || ! -f "$f" ]] && continue
    # Heuristic: skip binaries.
    if file --brief --mime "$f" | grep -q 'charset=binary'; then
      continue
    fi
    filtered+=("$f")
  done
fi

# Match against allow-list ---------------------------------------------------

is_allowed() {
  local f=$1
  for p in "${ALLOWED_PATHS[@]}"; do
    # shellcheck disable=SC2053
    if [[ "$f" == $p ]]; then
      return 0
    fi
  done
  return 1
}

in_ip_scope() {
  local f=$1
  for p in "${IP_SCOPED_GLOBS[@]}"; do
    # shellcheck disable=SC2053
    if [[ "$f" == $p ]]; then
      return 0
    fi
  done
  return 1
}

# Collect grep -n hits for one extended pattern, dropping lines that carry the
# opsec-allow annotation and (optionally) lines whose only match is an allowed
# value. Appends "file:N:line" to the global `violations` array.
scan_pattern() {
  local f=$1 pat=$2 allowed=${3:-}
  local line
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    # opsec-allow annotation exempts the line.
    [[ "$line" == *"$ANNOTATION"* ]] && continue
    # Allowlisted values (e.g. broadcast / all-zero MAC) that identify no gear.
    if [[ -n "$allowed" ]] && grep -qiE "$allowed" <<<"$line"; then
      continue
    fi
    violations+=("$f:$line")
  done < <(grep -n -E "$pat" "$f" 2>/dev/null || true)
}

# Scan ------------------------------------------------------------------------

violations=()
if (( ${#filtered[@]} > 0 )); then
  for f in "${filtered[@]}"; do
    is_allowed "$f" && continue
    # Operator-identity material (age / ssh): repo-wide.
    for pat in "${PATTERNS[@]}"; do
      while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        [[ "$line" == *"$ANNOTATION"* ]] && continue
        violations+=("$f:$line")
      done < <(grep -n -E "$pat" "$f" 2>/dev/null || true)
    done
    # Equipment MAC addresses: repo-wide, broadcast/all-zero allowed.
    scan_pattern "$f" "$MAC_PATTERN" "$MAC_ALLOWED_VALUES"
    # Mesh / CGNAT IPs: only in the portable surfaces.
    if in_ip_scope "$f"; then
      scan_pattern "$f" "$MESH_IP_PATTERN"
    fi
  done
fi

if [[ ${#violations[@]} -eq 0 ]]; then
  echo "ok: no operator-identity material detected in scanned files"
  exit 0
fi

echo "FAIL: opsec material in committed (non-gitignored) files"
echo "  Catches age recipients, SSH pubkeys, equipment MAC addresses, and"
echo "  mesh/CGNAT (100.64.0.0/10) IPs in portable surfaces. MACs and host"
echo "  mesh IPs are near-secrets (ADR 1196): keep them in a gitignored"
echo "  *.local.md / .yaml / .txt file or as op:// references, never tracked."
echo "  A legitimate non-live value (e.g. a synthetic MAC in a test) can carry"
echo "  an inline  opsec-allow: <reason>  annotation to pass."
echo "  Per CLAUDE.md \"OSS go-open strategy\" Block 30E,"
echo "  these belong in .envrc.local.d/40-dev-ca-operators.envrc (gitignored)."
echo
echo "  See docs/operations/local-secrets-pattern.md for the file layout."
echo
echo "Violations:"
for v in "${violations[@]}"; do
  echo "  $v"
done
exit 1
