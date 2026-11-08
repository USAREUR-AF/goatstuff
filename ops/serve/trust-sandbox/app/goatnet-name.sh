#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
#
# Runs from the nginx image's /docker-entrypoint.d before nginx starts.
# The built pages name the example goatnet (ZAGROS / zagros). TS_GOATNET names
# the goatnet this sandbox models; when it is set to another name, the pages
# are rewritten to carry it: upper case where they say ZAGROS, lower case
# where they say zagros (the URI-SAN examples and repository names).
set -eu
name="${TS_GOATNET:-}"
[ -n "$name" ] || exit 0
case "$name" in
  *[!a-zA-Z0-9-]*) echo "goatnet-name: TS_GOATNET must be letters, digits and '-'; pages left as built" >&2; exit 0 ;;
esac
lower=$(printf '%s' "$name" | tr 'A-Z' 'a-z')
upper=$(printf '%s' "$name" | tr 'a-z' 'A-Z')
[ "$lower" = "zagros" ] && exit 0
find /usr/share/nginx/html -type f \( -name '*.html' -o -name '*.js' \) \
  -exec sed -i -e "s/ZAGROS/${upper}/g" -e "s/zagros/${lower}/g" {} +
echo "goatnet-name: pages name ${upper}"
