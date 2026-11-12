#!/bin/bash
# Topology rig: eleven zenohd 1.9.0 routers on loopback (one container each, host
# network, digest-pinned image), goat-topology publishers on six, one serve on 8188.
# Build first (cargo build --release), then:
#   RIG_IMAGE=<router image@sha256:...> GOAT_TOPOLOGY_BIN=../target/release/goat-topology ./rig.sh up     # page at http://127.0.0.1:8188/
#   FIX=1 ./rig.sh restart-router bravo-gateway                        # give bravo a client subregion
#   ./rig.sh down
# Design doc zenoh-topology-surface.md §2.3-§2.4 records what it measured.
#
#
#   alpha-core-1..3   core, full mesh                    ours: publish
#   bravo-gateway     custom gateway, pods carved south  ours: publish (fallback endpoint)
#   bravo-pod-1/2     pods below bravo-gateway           ours: publish
#   charlie-gateway   custom gateway, pods carved south  PARTNER: no publisher
#   charlie-pod-1/2   pods below charlie-gateway         PARTNER: no publisher
#   delta-1           dials alpha-core-3                 PARTNER: no publisher
#   delta-2           dials delta-1 only (two hops out)  PARTNER: no publisher
set -u
D="$(cd "$(dirname "$0")" && pwd)"
IMG="${RIG_IMAGE:?set RIG_IMAGE to the digest-pinned fleet router image (fleet-pins zenoh-router); unpatched upstream eclipse/zenoh:1.9.0 wedged on one start here (lock cycle, upstream PR 2635)}"
T="${GOAT_TOPOLOGY_BIN:?set GOAT_TOPOLOGY_BIN}"
#        name             idx zid       connects  publish(site|-)  bravo gateway client subregion? (FIX=1)
ROUTERS="alpha-core-1     1  a1a1a1a1  -         alpha
alpha-core-2     2  a2a2a2a2  1         alpha
alpha-core-3     3  a3a3a3a3  1,2       alpha
bravo-gateway    4  b1b1b1b1  1,2       bravo
bravo-pod-1      5  b2b2b2b2  4         bravo
bravo-pod-2      6  b3b3b3b3  4         bravo
charlie-gateway  7  c1c1c1c1  2,3       -
charlie-pod-1    8  c2c2c2c2  7         -
charlie-pod-2    9  c3c3c3c3  7         -
delta-1          10 d1d1d1d1  3         -
delta-2          11 d2d2d2d2  10        -"
south() { case "$1" in
  bravo-gateway)   if [ -n "${FIX:-}" ]; then echo '{ filters: [ { zids: ["b2b2b2b2", "b3b3b3b3"] } ] }, { filters: [ { modes: ["client"] } ] }'
                   else echo '{ filters: [ { zids: ["b2b2b2b2", "b3b3b3b3"] } ] }'; fi;;
  charlie-gateway) echo '{ filters: [ { zids: ["c2c2c2c2", "c3c3c3c3"] } ] }, { filters: [ { modes: ["client"] } ] }';;
esac; }
conf() { # name idx zid connects
  local i s
  local eps=""; if [ "$4" != "-" ]; then for i in ${4//,/ }; do eps="$eps\"tcp/127.0.0.1:$((7450+i))\","; done; fi
  { echo "{"; echo "  id: \"$3\", mode: \"router\","
    echo "  listen: { endpoints: [\"tcp/127.0.0.1:$((7450+$2))\"] },"
    [ -n "$eps" ] && echo "  connect: { endpoints: [${eps%,}] },"
    echo "  scouting: { multicast: { enabled: false }, gossip: { enabled: false } },"
    echo "  plugins_loading: { enabled: true, search_dirs: [\"/\"] },"
    echo "  plugins: { rest: { http_port: \"127.0.0.1:$((8100+$2))\" } },"
    echo "  adminspace: { enabled: true, permissions: { read: true, write: false } },"
    s="$(south "$1")"; [ -n "$s" ] && echo "  gateway: { south: [ $s ] },"
    echo "}"; } > "$D/conf/$1.json5"
}
start_router() { conf "$1" "$2" "$3" "$4"
  docker rm -f "topo-$1" >/dev/null 2>&1
  docker run -d --name "topo-$1" --network host -v "$D/conf:/conf:ro" --entrypoint /zenohd "$IMG" -c "/conf/$1.json5" >/dev/null; }
# A gateway whose custom list admits no client refuses its own publisher too;
# its endpoint list names its own router first and a north neighbor second.
endpoints() { case "$1" in
  bravo-gateway) echo "tcp/127.0.0.1:7454,tcp/127.0.0.1:7451";;
  *) echo "tcp/127.0.0.1:$((7450+$2))";; esac; }
start_pub() { # name idx site
  nohup "$T" publish --endpoint "$(endpoints "$1" "$2")" --site "$3" --node "$1" \
    --admin-addr "127.0.0.1:$((8100+$2))" --config-path "$D/conf/$1.json5" --interval-secs 5 \
    > "$D/log/$1.publish.log" 2>&1 & echo $! > "$D/run/$1.publish.pid"; }
case "${1:-}" in
  up)   mkdir -p "$D/conf" "$D/log" "$D/run"
        echo "$ROUTERS" | while read n i z c p; do start_router $n $i $z $c; sleep 0.5; done; sleep 4
        echo "$ROUTERS" | while read n i z c p; do [ "$p" != "-" ] && start_pub $n $i $p; done
        nohup "$T" serve --endpoint tcp/127.0.0.1:7451 --bind 127.0.0.1:8188 > "$D/log/serve.log" 2>&1 & echo $! > "$D/run/serve.pid"
        echo "up: http://127.0.0.1:8188/";;
  restart-router) echo "$ROUTERS" | while read n i z c p; do [ "$n" = "$2" ] && {
          [ -f "$D/run/$n.publish.pid" ] && kill "$(cat "$D/run/$n.publish.pid")" 2>/dev/null
          start_router $n $i $z $c; sleep 3; [ "$p" != "-" ] && start_pub $n $i $p; }; done; echo "restarted $2";;
  down) for f in "$D"/run/*.pid; do [ -f "$f" ] && kill "$(cat "$f")" 2>/dev/null; rm -f "$f"; done
        docker ps -a --format '{{.Names}}' | grep '^topo-' | xargs -r docker rm -f >/dev/null; echo down;;
  *) echo "usage: rig.sh up|down|restart-router <name>   (FIX=1 adds a client subregion to bravo-gateway)";;
esac
