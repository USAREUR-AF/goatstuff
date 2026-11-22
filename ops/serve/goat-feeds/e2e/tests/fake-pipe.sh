#!/bin/sh
# Stand-in for goat-xf-pipe-zenoh in the browser tests: prints its arguments, then a counter
# line every second, as the real pipe does.
echo "goat-xf-pipe-zenoh: $*"
n=0
while :; do
  n=$((n + 5))
  echo "goat-xf-pipe-zenoh: in=$n converted=$n failed=0 ${n}s"
  sleep 1
done
