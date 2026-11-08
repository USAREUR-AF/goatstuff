#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
#
# Runs from the nginx image's /docker-entrypoint.d before nginx starts.
# The built pages carry __GOATNET__ / __goatnet__ where they name the goatnet
# the sandbox models. TS_GOATNET names it (unset: "goatnet", which names no
# deployment): upper case for __GOATNET__, lower case for __goatnet__ (the
# URI-SAN examples and repository names).
set -eu
name="${TS_GOATNET:-goatnet}"
case "$name" in
  *[!a-zA-Z0-9-]*) echo "goatnet-name: TS_GOATNET must be letters, digits and '-'; using goatnet" >&2; name=goatnet ;;
esac
lower=$(printf '%s' "$name" | tr 'A-Z' 'a-z')
upper=$(printf '%s' "$name" | tr 'a-z' 'A-Z')
find /usr/share/nginx/html -type f \( -name '*.html' -o -name '*.js' \) \
  -exec sed -i -e "s/__GOATNET__/${upper}/g" -e "s/__goatnet__/${lower}/g" {} +
echo "goatnet-name: pages name ${upper}"
