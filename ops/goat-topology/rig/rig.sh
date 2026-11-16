#!/bin/bash
# Topology rig: twenty-two zenohd 1.9.0 routers on loopback (one container each,
# host network, digest-pinned image), goat-topology publishers on six, one serve
# on 8188, and rig clients that declare a known number of subscribers and
# publishers on each of seven routers. Build first:
#   cargo build --release && cargo build --release --example rig-client
#   RIG_IMAGE=<router image@sha256:...> GOAT_TOPOLOGY_BIN=../target/release/goat-topology \
#     RIG_CLIENT_BIN=../target/release/examples/rig-client ./rig.sh up    # page at http://127.0.0.1:8188/
#   ./rig.sh check                       # the page's names and counts against what the rig declared
#   FIX=1 ./rig.sh restart-router bravo-gateway                        # give bravo a client subregion
#   ./rig.sh down
# Design doc zenoh-topology-surface.md §2.3-§2.5 records what it measured.
#
# Each router listens on, and dials from, its own loopback address 127.0.0.<10+idx>,
# so a session's address says which router it is, as a mesh address does on a
# goatnet. conf/peer-names.json maps those addresses to synthetic peer names in
# the relay-health peer-names shape; conf/roster.json is a synthetic roster.
#
#   alpha-core-1..3   core, full mesh                    ours: publish
#   bravo-gateway     custom gateway, pods carved south  ours: publish (fallback endpoint)
#   bravo-pod-1/2     pods below bravo-gateway           ours: publish
#   charlie-gateway   custom gateway, pods carved south  PARTNER: no publisher
#   charlie-pod-1/2   pods below charlie-gateway         PARTNER: no publisher
#   delta-1           dials alpha-core-3                 PARTNER: no publisher
#   delta-2           dials delta-1 only (two hops out)  PARTNER: no publisher, no peer name seen
#   echo-1..11        dial alpha-core-1 or -2            PARTNER: no publisher, long peer names
set -u
D="$(cd "$(dirname "$0")" && pwd)"
IMG="${RIG_IMAGE:-}"
T="${GOAT_TOPOLOGY_BIN:-}"
C="${RIG_CLIENT_BIN:-}"
need() { [ -n "$IMG" ] || { echo "set RIG_IMAGE to the digest-pinned fleet router image (fleet-pins zenoh-router); unpatched upstream eclipse/zenoh:1.9.0 wedged on one start here (lock cycle, upstream PR 2635)"; exit 2; }
  [ -n "$T" ] || { echo "set GOAT_TOPOLOGY_BIN"; exit 2; }; }
#        name             idx zid       connects  publish  subs pubs  peer name (for a router that sends no report)
ROUTERS="alpha-core-1     1  a1a1a1a1  -         alpha    540  50  -
alpha-core-2     2  a2a2a2a2  1         alpha    0    0   -
alpha-core-3     3  a3a3a3a3  1,2       alpha    3    2   -
bravo-gateway    4  b1b1b1b1  1,2       bravo    0    0   -
bravo-pod-1      5  b2b2b2b2  4         bravo    0    0   -
bravo-pod-2      6  b3b3b3b3  4         bravo    0    0   -
charlie-gateway  7  c1c1c1c1  2,3       -        4    1   machine-charlie-gw-forward-relay-rack-b
charlie-pod-1    8  c2c2c2c2  7         -        2    1   -
charlie-pod-2    9  c3c3c3c3  7         -        0    0   -
delta-1          10 d1d1d1d1  3         -        7    3   peer-okafor-n-zagros-2026-10-02-field-laptop-07
delta-2          11 d2d2d2d2  10        -        5    0   -
echo-1           12 e1e1e1e1  1         -        1    0   machine-anvil-pod-7-rack2
echo-2           13 e2e2e2e2  2         -        0    0   machine-cedar-relay
echo-3           14 e3e3e3e3  1         -        6    2   peer-rivera-j-2026-09-30-desk
echo-4           15 e4e4e4e4  2         -        0    0   machine-unlisted-tracking-cell-observer-node-0042
echo-5           16 e5e5e5e5  1         -        0    0   partner-gateway-without-a-grammar-name
echo-6           17 e6e6e6e6  2         -        0    0   machine-anvil-pod-7-rack3
echo-7           18 e7e7e7e7  1         -        0    0   machine-birch-sensor-hub-north-quadrant-12
echo-8           19 e8e8e8e8  2         -        0    0   machine-birch-sensor-hub-south-quadrant-14
echo-9           20 e9e9e9e9  1         -        0    0   machine-anvil-long-haul-relay-03
echo-10          21 eaeaeaea  2         -        0    0   machine-cedar-relay-standby
echo-11          22 ebebebeb  1         -        0    0   -"
addr() { echo "127.0.0.$((10 + $1))"; }
south() { case "$1" in
  bravo-gateway)   if [ -n "${FIX:-}" ]; then echo '{ filters: [ { zids: ["b2b2b2b2", "b3b3b3b3"] } ] }, { filters: [ { modes: ["client"] } ] }'
                   else echo '{ filters: [ { zids: ["b2b2b2b2", "b3b3b3b3"] } ] }'; fi;;
  charlie-gateway) echo '{ filters: [ { zids: ["c2c2c2c2", "c3c3c3c3"] } ] }, { filters: [ { modes: ["client"] } ] }';;
esac; }
conf() { # name idx zid connects
  local i s
  local eps=""; if [ "$4" != "-" ]; then for i in ${4//,/ }; do eps="$eps\"tcp/$(addr "$i"):$((7450+i))#bind=$(addr "$2"):0\","; done; fi
  { echo "{"; echo "  id: \"$3\", mode: \"router\","
    echo "  listen: { endpoints: [\"tcp/$(addr "$2"):$((7450+$2))\"] },"
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
  bravo-gateway) echo "tcp/$(addr 4):7454,tcp/$(addr 1):7451";;
  *) echo "tcp/$(addr "$2"):$((7450+$2))";; esac; }
start_pub() { # name idx site
  nohup "$T" publish --endpoint "$(endpoints "$1" "$2")" --site "$3" --node "$1" \
    --admin-addr "127.0.0.1:$((8100+$2))" --config-path "$D/conf/$1.json5" --interval-secs 5 \
    > "$D/log/$1.publish.log" 2>&1 & echo $! > "$D/run/$1.publish.pid"; }
start_client() { # name idx subs pubs
  [ -n "$C" ] || return 0
  [ "$3" = 0 ] && [ "$4" = 0 ] && return 0
  nohup "$C" "tcp/$(addr "$2"):$((7450+$2))" "$1" "$3" "$4" > "$D/log/$1.client.log" 2>&1 & echo $! > "$D/run/$1.client.pid"; }
# The synthetic identity inputs: a peer-names file keyed as relay-health keys
# its own, and a roster in the identity registry's public-safe shape.
identity() {
  { echo '{"names": {'; local first=1
    echo "$ROUTERS" | while read n i z c p s q name; do
      [ "$name" = "-" ] && continue
      [ $first = 1 ] || printf ','; first=0
      printf '"sha-%s": {"name": "%s", "ip": "%s", "host": "%s-host", "aff": "ZAGROS-PARTNER,ZAGROS-OBSERVER"}\n' "$z" "$name" "$(addr "$i")" "$n"
    done
    echo '}, "generated_at": 0}'; } > "$D/conf/peer-names.json"
  cat > "$D/conf/roster.json" <<'EOF'
{"orgs": [
  {"id": "org-anvil", "nation": "XAA", "display_name": "Anvil Works", "members": [
    {"handle": "rivera-j", "type": "person"},
    {"handle": "anvil-pod-7", "type": "system", "custody": "rivera-j"},
    {"handle": "anvil-long-haul-relay-03", "type": "system"}]},
  {"id": "org-birch", "nation": "XBB", "display_name": "Birch Field Sensing Consortium", "members": [
    {"handle": "okafor-n", "type": "person"},
    {"handle": "birch-sensor-hub-north-quadrant-12", "type": "system", "custody": "okafor-n"},
    {"handle": "birch-sensor-hub-south-quadrant-14", "type": "system", "custody": "okafor-n"}]},
  {"id": "org-cedar", "members": [
    {"handle": "cedar-relay", "type": "system"},
    {"handle": "charlie-gw-forward-relay", "type": "system", "custody": "rivera-j"}]}
 ],
 "members": [
  {"handle": "rivera-j", "type": "person", "org": "org-anvil", "nationality": "XCC"},
  {"handle": "anvil-pod-7", "type": "system", "org": "org-anvil"},
  {"handle": "anvil-long-haul-relay-03", "type": "system", "org": "org-anvil"},
  {"handle": "okafor-n", "type": "person", "org": "org-birch", "nationality": null},
  {"handle": "birch-sensor-hub-north-quadrant-12", "type": "system", "org": "org-birch"},
  {"handle": "birch-sensor-hub-south-quadrant-14", "type": "system", "org": "org-birch"},
  {"handle": "cedar-relay", "type": "system", "org": "org-cedar"},
  {"handle": "charlie-gw-forward-relay", "type": "system", "org": "org-cedar"}
 ]}
EOF
  echo "$ROUTERS" | awk '{print $1, $2, $3, $5, $6, $7, $8}' > "$D/conf/declared.txt"; }
case "${1:-}" in
  up)   need; mkdir -p "$D/conf" "$D/log" "$D/run"; identity
        echo "$ROUTERS" | while read n i z c p s q name; do start_router $n $i $z $c; sleep 0.5; done; sleep 4
        echo "$ROUTERS" | while read n i z c p s q name; do [ "$p" != "-" ] && start_pub $n $i $p; done
        echo "$ROUTERS" | while read n i z c p s q name; do start_client $n $i $s $q; done
        nohup "$T" serve --endpoint "tcp/$(addr 1):7451" --bind 127.0.0.1:8188 \
          --peer-directory "$D/conf/peer-names.json" --roster "$D/conf/roster.json" --goatnet zagros \
          > "$D/log/serve.log" 2>&1 & echo $! > "$D/run/serve.pid"
        echo "up: http://127.0.0.1:8188/";;
  check) python3 -I "$D/check.py" "$D/conf/declared.txt" "$D/conf/peer-names.json" "http://127.0.0.1:8188/api/topology";;
  restart-router) need; echo "$ROUTERS" | while read n i z c p s q name; do [ "$n" = "$2" ] && {
          [ -f "$D/run/$n.publish.pid" ] && kill "$(cat "$D/run/$n.publish.pid")" 2>/dev/null
          start_router $n $i $z $c; sleep 3; [ "$p" != "-" ] && start_pub $n $i $p; }; done; echo "restarted $2";;
  down) for f in "$D"/run/*.pid; do [ -f "$f" ] && kill "$(cat "$f")" 2>/dev/null; rm -f "$f"; done
        docker ps -a --format '{{.Names}}' | grep '^topo-' | xargs -r docker rm -f >/dev/null; echo down;;
  *) echo "usage: rig.sh up|check|down|restart-router <name>   (FIX=1 adds a client subregion to bravo-gateway)";;
esac
