#!/usr/bin/env bash
# goat-map must not name a site outside deploy/goatnets/<goatnet>.env.
#
# The repo's own rule (README, "Deploying"): "Every goatnet-specific value
# lives in one file, deploy/goatnets/<goatnet>.env. Nothing else in this repo
# names a site, host, zone or sensor." A basemap filename hardcoded in code
# makes every goatnet open its map over one site's ground.
#
# The site names and centres it refuses are read from those env files, so the
# guard knows every declared site without naming one itself.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MAP="$ROOT/ops/serve/goat-map"
fail=0
scan() { # <regex> <human reason>
  local hits
  # Strip the `path:lineno:` prefix before deciding whether the HIT ITSELF is
  # a comment — otherwise `^//` never matches and the guard flags the very
  # comments that explain it. tests/ carry geometry fixtures at these
  # coordinates on purpose and are not a deploy path.
  hits=$(grep -rnE "$1" "$MAP" \
      --include=*.js --include=*.py --include=*.json --include=*.html \
      --exclude-dir=vendor --exclude-dir=basemap --exclude-dir=tests 2>/dev/null \
      | awk -F: '{ line=$0; sub(/^[^:]*:[0-9]+:/, "", line);
                   gsub(/^[ \t]+/, "", line);
                   if (line !~ /^(\/\/|#|\*)/) print }' || true)
  if [ -n "$hits" ]; then
    echo "REFUSED: $2" >&2; echo "$hits" >&2; fail=1
  fi
}
ENVS=("$MAP"/deploy/goatnets/*.env)
[ -e "${ENVS[0]}" ] || { echo "OK: no deploy/goatnets/*.env declares a site"; exit 0; }
# A basemap filename is a site name. Config carries it; code must not.
basemaps=$(sed -n 's/^GOATMAP_BASEMAP=//p' "${ENVS[@]}" | tr -d '"' | sed 's#.*/##; s/\./\\./g' | sort -u | paste -sd'|' -)
[ -n "$basemaps" ] && scan "($basemaps)" \
  "a basemap filename is hardcoded in code — set GOATMAP_BASEMAP in deploy/goatnets/<goatnet>.env instead"
# A declared map centre, to two decimals, either order and either array or pair form.
while IFS=, read -r a b _; do
  a=$(printf '%s' "$a" | tr -dc '0-9.-' | sed -E 's/^(-?[0-9]+\.[0-9]{2}).*/\1/; s/\./\\./')
  b=$(printf '%s' "$b" | tr -dc '0-9.-' | sed -E 's/^(-?[0-9]+\.[0-9]{2}).*/\1/; s/\./\\./')
  [ -n "$a" ] && [ -n "$b" ] || continue
  scan "${a}[0-9]*\\s*,\\s*${b}|${b}[0-9]*\\s*,\\s*${a}" \
    "a site coordinate is hardcoded in code — set GOATMAP_CENTER in deploy/goatnets/<goatnet>.env instead"
done < <(sed -n 's/^GOATMAP_CENTER=//p' "${ENVS[@]}" | tr -d '"[] ' | sort -u)
[ "$fail" -eq 0 ] && echo "OK: goat-map names no site outside deploy/goatnets/"
exit "$fail"
