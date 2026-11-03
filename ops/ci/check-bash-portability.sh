#!/usr/bin/env bash
# ops/ci/check-bash-portability.sh — static guard for Bash builtin portability.
#
# macOS ships /bin/bash 3.2, which lacks several newer builtins and syntax:
#   - mapfile / readarray
#   - associative arrays (declare -A / typeset -A, any combined flag form)
#   - namerefs (local -n / declare -n / typeset -n)
#   - case-conversion expansions (${var,,} ${var^^} ${var,} ${var^})
#   - the |& pipe shorthand
#
# A script that runs under set -euo pipefail can pass `bash -n` syntax checks
# while still failing at runtime on macOS when it hits a missing builtin. This
# guard statically scans the tracked CI and operator scripts for those tokens
# in code (not comments) and fails on any occurrence. That is how we enforce
# that the ops shell tree — every tracked *.sh under ops/ plus the
# extensionless operator entry points — remains runnable under both macOS
# /bin/bash 3.2 and Bash 5.x. (#227: discovery originally covered only
# ops/ci and ops/scripts, the narrower of #205's stated goal; twelve files
# elsewhere carried bash-4-only constructs — the nine operator-run ones
# died on stock macOS, the three host-deployed ones are declared below.)
#
# Scope: the gate exists for scripts that can run on an operator's macOS
# laptop. Scripts that only ever run on goat hosts (Linux, bash 5.x) are
# declared in LINUX_HOST_ONLY below and skipped — forcing 3.2 idioms onto
# them buys nothing and costs readability.
#
# References:
#   - check-no-identity-spillage.sh — prior-art guard whose behavior tests
#     live in tests/check-no-identity-spillage.bats; this script provides the
#     static 3.2 portability gate that those tests do not cover.
#
# Known limitation (accepted, line-based scan without a shell parser): a `#`
# inside a quoted string still reads as a comment start when whitespace
# precedes it, so a forbidden token AFTER such a string on the same line is
# missed. The miss direction is false-negative only; every fail-closed
# direction (tokens in strings, substring name collisions) over-matches
# instead.
#
# Usage:
#   ops/ci/check-bash-portability.sh            # scan all tracked ops/**/*.sh + operator entry points
#   ops/ci/check-bash-portability.sh path ...   # scan exactly the listed files
#
# Exit: 0 clean · 1 violation (file:line listed) · 2 usage/dep error
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO_ROOT"

# One ERE, four construct classes:
#   mapfile/readarray            — bash 4.0 builtins, matched anywhere (a
#                                  variable merely NAMED mapfile_x also trips;
#                                  fail-closed, rename it)
#   declare/typeset/local -…A/n… — associative arrays and namerefs, including
#                                  combined flag forms (-rA, -gA, -Ax, …)
#   ${var,,} ${var^^} etc.       — bash 4.0 case-conversion expansions,
#                                  including on an indexed element
#   |&                           — bash 4.0 shorthand for 2>&1 |
pattern='mapfile|readarray|(declare|typeset|local)[[:space:]]+-[A-Za-z]*[An]|\$\{[A-Za-z_][A-Za-z0-9_]*(\[[^]]+\])?[,^]|\|&'

# Scripts that run only on goat hosts (Linux, bash 5.x) — never on operator
# macOS. One repo-relative path per line.
LINUX_HOST_ONLY='
ops/scripts/vxlan-rca.sh
ops/ansible/roles/netbird-admin-reconcile/files/netbird-admin-reconcile.sh
ops/ansible/roles/spock-drift-watch/files/spock-drift-watch.sh
ops/ansible/roles/ory/tests/soak/repro-spock-kratos.sh
'

# Extensionless entry points are discovered from tracked files and their
# shebangs. Renaming or adding one must not require a second hand-kept list.
has_bash_shebang() {
  head -n 1 "$1" | grep -Eq '^#!.*(bash)([[:space:]]|$)'
}

is_linux_host_only() {
  printf '%s\n' "$LINUX_HOST_ONLY" | grep -Fxq -- "$1"
}

files=()
if [ "$#" -eq 0 ]; then
  # Every tracked shell script under ops/, plus the extensionless
  # operator entry points (#227 — the old ops/ci + ops/scripts scope let
  # bash-4-only builtins land in operator scripts elsewhere).
  while IFS= read -r f; do
    files+=("$f")
  done < <(git ls-files 'ops/*.sh')
  while IFS= read -r f; do
    case "$(basename "$f")" in *.*) continue ;; esac
    [ -f "$f" ] || { echo "FAIL: tracked file '$f' does not exist." >&2; exit 2; }
    if has_bash_shebang "$f"; then
      files+=("$f")
    fi
  done < <(git ls-files ops)
else
  files=("$@")
fi

if [ "${#files[@]}" -eq 0 ]; then
  echo "FAIL: no files to scan — are we in a git worktree?" >&2
  exit 2
fi

fail=0
missing=0
for f in "${files[@]}"; do
  # Skip this script itself so it can carry the forbidden tokens in comments.
  base="$(basename "$f")"
  if [ "$base" = "check-bash-portability.sh" ]; then
    continue
  fi
  if is_linux_host_only "$f"; then
    echo "skip (linux-host-only, declared above): $f"
    continue
  fi
  if [ ! -f "$f" ]; then
    echo "FAIL: file '$f' does not exist." >&2
    missing=1
    continue
  fi

  # Pre-filter with one grep per file, then confirm each hit survives
  # comment stripping. A `#` begins a comment only at the start of a word,
  # so strip from a whitespace-preceded `#` (whole-line comments are skipped
  # outright) — NOT from the first `#` on the line, which would truncate
  # `${#arr[@]}` and hide any forbidden token after it.
  while IFS=: read -r lineno line; do
    # Skip lines that are pure comments once leading whitespace is stripped.
    stripped="${line#"${line%%[![:space:]]*}"}"
    case "$stripped" in \#*) continue ;; esac

    code="$(printf '%s\n' "$line" | sed -E 's/[[:space:]]#.*$//')"
    if printf '%s\n' "$code" | grep -qE "$pattern"; then
      printf '%s:%s: %s\n' "$f" "$lineno" "$code"
      fail=1
    fi
  done < <(grep -nE "$pattern" "$f" || true)
done

if [ "$missing" -ne 0 ]; then
  exit 2
fi

if [ "$fail" -ne 0 ]; then
  echo "FAIL: bash portability guard found forbidden bash-4-only usage in CI/operator scripts." >&2
  echo "      Forbidden: mapfile, readarray, declare/typeset/local -A or -n (any flag" >&2
  echo "      combination), \${var,,}-style case conversion, and |&." >&2
  exit 1
fi

echo "ok: no bash-4-only usage in the ops shell tree (portable to macOS /bin/bash 3.2)."
