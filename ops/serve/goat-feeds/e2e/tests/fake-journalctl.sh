#!/bin/sh
# Stands in for `journalctl -u <unit> -n N -o json` in the browser tests: two status lines,
# ten seconds apart, with the input count moving, so the link reads as sending.
now=$(date +%s)
n=$((now % 100000))
printf '{"__REALTIME_TIMESTAMP":"%s000000","MESSAGE":"goat-xf-pipe-zenoh: in=%s converted=%s failed=0 10s"}\n' "$((now - 10))" "$n" "$((n * 2))"
printf '{"__REALTIME_TIMESTAMP":"%s000000","MESSAGE":"goat-xf-pipe-zenoh: in=%s converted=%s failed=0 20s"}\n' "$now" "$((n + 40))" "$((n * 2 + 80))"
