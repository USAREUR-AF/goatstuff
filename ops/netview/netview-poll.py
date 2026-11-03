#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
#
# netview-poll — Prometheus -> one JSON for the netview pages (ADR 1128).
#
# WHY A POLLER AND NOT A BROWSER FETCH: the pages are static files behind
# oauth2-proxy on the relay-health nginx. Letting the browser query Prometheus
# would mean exposing Prometheus (it is loopback-only, deliberately) and would
# put client demand on a query path per viewer. This runs once per interval on
# the host, writes one document, and every viewer reads the same bytes — the
# ADR 1123 shape: client demand must not amplify into backend work.
#
# HONESTY RULES, enforced here rather than in the page:
#   - a metric with no series is null, never 0. The page renders null as
#     "not measured" and 0 as zero, and they are different facts.
#   - `capacity` is null when nothing declared one; percent fields are then
#     null too, never a number derived from a guess.
#   - every reading carries the query that produced it, so a number on the
#     page can be traced without reading this file.
#
# Written to <out>.tmp then renamed, so nginx never serves a half-written doc.

import argparse
import json
import math
import os
import sys
import tempfile
import time
import urllib.parse
import urllib.request

DEFAULT_PROM = "http://127.0.0.1:9192"

# RI — rate window for TRAEFIK/node-exporter series. Those are scraped every
# 15-30s, so a 5m window holds 10-20 samples and rate() barely extrapolates.
RI = "5m"

# SOCKET_RI — rate window for anything derived from node-net-health's socket
# attribution (goat_net_workload_bytes_total, goat_net_transport_bytes_total).
#
# THESE ARE NOT SCRAPED SERIES. They are a textfile counter rewritten by
# node-net-health-attribution.service, which takes ~120-145s per run scanning
# ~400 sockets and restarts immediately, so the counter increments roughly every
# 150s regardless of what the timer's OnUnitActiveSec says.
#
# A 5m rate() window therefore holds about TWO increments. Prometheus rate()
# extrapolates the observed delta out to the full window, and with two samples
# that inflates the result by up to ~2x — worst exactly when the deltas are
# largest, i.e. under load. The result is a transport-underlay class reading
# above the physical NIC, which is impossible (TCP payload cannot exceed the
# frames carrying it) and is entirely
# this artifact. The counter is CORRECT; comparing its 5m rate against a
# 15s-scraped NIC rate is not.
#
# 20m holds ~8 increments, which brings the extrapolation error into the noise.
# The honest alternative is making the job fast enough to increment every 60s;
# until then the window has to respect the sampling rate that actually exists.
SOCKET_RI = "20m"


def q(prom, expr, timeout=10):
    """One instant query -> list of (labels, float). [] when nothing matches."""
    url = f"{prom}/api/v1/query?" + urllib.parse.urlencode({"query": expr})
    try:
        with urllib.request.urlopen(url, timeout=timeout) as r:
            body = json.load(r)
    except Exception as e:
        print(f"netview-poll: query failed: {e}", file=sys.stderr)
        return None                      # None = COULD NOT ASK, distinct from []
    if body.get("status") != "success":
        return None
    out = []
    for s in body["data"]["result"]:
        try:
            v = float(s["value"][1])
        except (ValueError, KeyError):
            continue
        if math.isnan(v):                # NaN: an empty histogram, not a value
            continue
        out.append(({k: v2 for k, v2 in s["metric"].items() if k != "__name__"}, v))
    return out


def scalar(prom, expr):
    r = q(prom, expr)
    if r is None or not r:
        return None                      # absent stays absent all the way up
    return r[0][1]


def series(prom, expr, key):
    r = q(prom, expr)
    if r is None:
        return None
    return sorted(({"name": m.get(key, "?"), "value": v, **m} for m, v in r),
                  key=lambda d: -d["value"])


def canary(prom):
    """Surface canary, two planes, joined per surface (ADR 1133).

    The join is by the `surface` label both blackbox jobs carry, so a service
    appears once with an app reading and a gate reading side by side. They are
    NEVER combined into one number: they measure different things through
    different amounts of infrastructure, and averaging them would reproduce the
    exact mistake this is here to fix.

    The verdict vocabulary is deliberately small and each word means one thing:

      up            app answered; gate answered as the surface itself (200).
                    The only state that means "a user can load this".
      gate-only     app answered; gate answered as the AUTH GATE (302). The
                    surface is healthy and the front door is alive, but nothing
                    here proves a logged-in user gets a working page — the
                    probe has no credential. Not a warning, an honest limit.
      surface-down  the gate is fine but the app itself is not answering. This
                    is the state a gate-only probe would have MISSED, and the
                    reason the app plane exists.
      unreachable   the gate did not answer at all: DNS, mesh path, TLS or
                    Traefik. The surface may be perfectly healthy behind it.
      unmeasured    no series on either plane. Never rendered as a colour.
    """
    def by_surface(expr, field):
        r = q(prom, expr)
        if r is None:
            return {}
        out = {}
        for m, v in r:
            s = m.get("surface")
            if s:
                out.setdefault(s, {})[field] = v
        return out

    merged = {}
    for expr, field in (
        ('probe_success{job="blackbox-surface-app"}', "app_up"),
        ('probe_duration_seconds{job="blackbox-surface-app"}', "app_s"),
        ('probe_success{job="blackbox-https"}', "gate_up"),
        ('probe_duration_seconds{job="blackbox-https"}', "gate_s"),
        ('probe_http_status_code{job="blackbox-https"}', "gate_code"),
    ):
        for s, vals in by_surface(expr, field).items():
            merged.setdefault(s, {}).update(vals)

    rows = []
    for name in sorted(merged):
        v = merged[name]
        app_up = v.get("app_up")
        gate_up = v.get("gate_up")
        code = v.get("gate_code")
        code = int(code) if code is not None else None

        if app_up is None and gate_up is None:
            verdict = "unmeasured"
        elif gate_up == 0:
            verdict = "unreachable"
        elif app_up == 0:
            verdict = "surface-down"
        elif app_up is None:
            # External services (goat-map, trust-sandbox) have no container to
            # probe, so the gate is all there is. A 200 there IS the surface
            # answering — those health paths are genuinely unauthenticated.
            verdict = "up" if code == 200 else "gate-only"
        else:
            verdict = "up" if code == 200 else "gate-only"

        rows.append({
            "surface": name,
            "verdict": verdict,
            "app_ms": (v["app_s"] * 1000) if v.get("app_s") is not None else None,
            "gate_ms": (v["gate_s"] * 1000) if v.get("gate_s") is not None else None,
            "gate_code": code,
            "app_probed": app_up is not None,
        })

    order = {"unreachable": 0, "surface-down": 1, "gate-only": 2, "up": 3, "unmeasured": 4}
    rows.sort(key=lambda r: (order.get(r["verdict"], 9), -(r["app_ms"] or 0)))
    return {
        "surfaces": rows,
        "n_up": sum(1 for r in rows if r["verdict"] == "up"),
        "n_gate_only": sum(1 for r in rows if r["verdict"] == "gate-only"),
        "n_bad": sum(1 for r in rows if r["verdict"] in ("unreachable", "surface-down")),
        "n_total": len(rows),
        # Said out loud rather than left for someone to derive from a scrape
        # config: what this costs the fabric. /healthz on every surface is a
        # constant handler, so at this rung the canary runs NO fabric query and
        # applies no fabric pressure (ADR 1123). The moment a deep probe is
        # added that stops being true, and this line has to change with it.
        "app_interval_s": 30,
        "exercises_fabric": False,
    }


def build(prom, host_filter, grammar):
    H = f'host=~"{host_filter}"'
    now = int(time.time())

    # ── shed policy, read from the grammar so there is ONE home for it ──────
    shed, essential = [], []
    for c in grammar.get("workload", []):
        if c.get("criticality") == "essential":
            essential.append(c["key"])
        if c.get("shed_order", 0) >= 5:
            shed.append(c)
    shed.sort(key=lambda c: -c["shed_order"])
    shed_re = "|".join(c["key"] for c in shed) or "__none__"
    ess_re = "|".join(essential) or "__none__"
    # Not workload: counted elsewhere or bookkeeping. Excluding these is what
    # keeps the two planes from being summed.
    NOT_WORKLOAD = 'category!~"http-delegated|transport-underlay"'

    d = {
        "generated_unix": now,
        "generated_iso": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(now)),
        "host_filter": host_filter,
        "prom": prom,
    }

    # ── Q1: how full is the pipe ────────────────────────────────────────────
    d["pipe"] = {
        "nic_rx_bps": scalar(prom, f'sum(goat:link_bits_per_second:rx{{{H}}})'),
        "nic_tx_bps": scalar(prom, f'sum(goat:link_bits_per_second:tx{{{H}}})'),
        "capacity_rx_bps": scalar(
            prom, f'max(goat_link_capacity_bytes_per_second{{{H},direction="rx"}}) * 8'),
        "capacity_tx_bps": scalar(
            prom, f'max(goat_link_capacity_bytes_per_second{{{H},direction="tx"}}) * 8'),
        "util_rx": scalar(prom, f'max(goat:link_utilisation_ratio:rx{{{H}}})'),
        "util_tx": scalar(prom, f'max(goat:link_utilisation_ratio:tx{{{H}}})'),
        "hosts_undeclared": scalar(prom, 'count(goat_link_capacity_declared == 0)'),
        "hosts_reporting": scalar(prom, f'count(goat_net_attribution_up{{{H}}})'),
    }

    # ── transport plane: what the physical pipe carries ─────────────────────
    tr = {}
    for cls in ("mesh-p2p", "mesh-relayed"):
        for dirn in ("rx", "tx"):
            tr[f"{cls}_{dirn}_bps"] = scalar(
                prom, f'sum(rate(goat_net_transport_bytes_total{{{H},class="{cls}",'
                      f'direction="{dirn}"}}[{SOCKET_RI}])) * 8')
    mesh_rx = scalar(prom, f'sum(rate(goat_net_transport_bytes_total{{{H},direction="rx"}}[{SOCKET_RI}])) * 8')

    # THE NIC, MEASURED OVER THE SAME WINDOW AS THE THINGS COMPARED AGAINST IT.
    #
    # d["pipe"]["nic_rx_bps"] comes from the recording rule
    # goat:link_bits_per_second:rx, which is a rate() over [5m]. Every socket
    # series below is a rate() over SOCKET_RI (20m). Subtracting one from the
    # other, or calling the difference an inconsistency, compares two different
    # questions: "how fast in the last 5 minutes" and "how fast in the last 20".
    #
    # They agree while load is flat and diverge exactly when it is not. When load
    # falls, NIC over [5m] reads below [1h]. In that state a 20m socket average NECESSARILY exceeds a 5m NIC average,
    # and the page reported "tunnel class exceeds NIC, treat both as unreliable"
    # for what is nothing more than the two windows disagreeing about when.
    #
    # The headline pipe number stays on the recording rule: it is what the
    # interface is doing NOW, which is the right question for a headline. This
    # one is for arithmetic only, where the sole requirement is that both sides
    # were asked about the same interval.
    nic_rx = scalar(
        prom, f'sum(rate(node_network_receive_bytes_total{{{H},'
              f'device!~"lo|wt0|nb-.*|veth.*|docker.*|br-.*|virbr.*|tun.*|tap.*|wg.*"}}'
              f'[{SOCKET_RI}])) * 8')
    # Fall back to the headline if the raw series is unavailable, rather than
    # dropping the whole transport plane. Flagged so the page can say which it used.
    tr["nic_window_matched"] = nic_rx is not None
    if nic_rx is None:
        nic_rx = d["pipe"]["nic_rx_bps"]
    # Emitted so the page divides by the SAME number this arithmetic used. The
    # ratio was previously taken against the 5m headline while the numerator was
    # a 20m socket average, which is what produced "126%" and then
    # "tunnel exceeds NIC" — a unit mismatch rendered as a measurement.
    tr["nic_rx_bps"] = nic_rx

    # RELAY TUNNEL AS A MEASURED CLASS, not part of the remainder.
    #
    # This was the flaw that made the chart useless: rathole (:2333, the relay
    # ingress tunnels, ADR 1085) is ~94% of this host's pipe and it IS measured
    # — the socket counters have it. But it was categorised into the WORKLOAD
    # plane as transport-underlay and then excluded from the workload stack,
    # so it fell out of both views and reappeared as an unexplained "derived
    # remainder". A visualisation that is 94% remainder is not a measurement.
    #
    # It is transport, which is why it is named transport-underlay, so it goes
    # in the transport plane where it belongs, which leaves the derived
    # remainder a genuine residual instead of a blob.
    tr["relay_tunnel_rx_bps"] = scalar(
        prom, f'sum(rate(goat_net_workload_bytes_total{{{H},'
              f'category="transport-underlay",direction="rx",netns="host"}}'
              f'[{SOCKET_RI}])) * 8')

    # Whatever is left after every MEASURED class. Kept, and kept honest: it is
    # the only row on the page nobody can name, so it must stay visible and it
    # must stay small. If it grows, something new is on the wire.
    known = sum(v for v in (mesh_rx, tr["relay_tunnel_rx_bps"]) if v is not None)
    tr["other_rx_bps"] = (max(0.0, nic_rx - known) if nic_rx is not None else None)
    tr["other_share"] = ((tr["other_rx_bps"] / nic_rx)
                         if (nic_rx not in (None, 0) and tr["other_rx_bps"] is not None) else None)
    tr["mesh_total_rx_bps"] = mesh_rx
    # THIS NODE'S OWN WORK, as a scalar, so the page can state the transit-vs-work
    # split as a sentence instead of leaving a reader to derive it from a bar.
    #
    # An operator looking at a relay-ingress host sees rathole dominating the
    # chart and cannot tell from that whether the machine is busy or idle. It is
    # neither: the tunnel is OTHER PEERS' traffic passing THROUGH (ADR 1085),
    # and this node's own work is a rounding error beside it.
    # Both numbers are needed to say that, so both are emitted.
    tr["own_workload_rx_bps"] = scalar(
        prom, f'sum(rate(goat_net_workload_bytes_total{{{H},{NOT_WORKLOAD},'
              f'direction="rx",netns="host"}}[{SOCKET_RI}])) * 8')
    tr["relayed_share"] = scalar(
        prom, f'sum(rate(goat_net_transport_bytes_total{{{H},class="mesh-relayed"}}[{SOCKET_RI}])) '
              f'/ clamp_min(sum(rate(goat_net_transport_bytes_total{{{H}}}[{SOCKET_RI}])), 1)')
    # ── PQC cost, made visible instead of hiding inside "signal" ────────────
    #
    # F-338: a Rosenpass v1 static public key is Classic McEliece 460896 —
    # 524,160 bytes — and netbird puts the whole key in every signal OFFER and
    # ANSWER. On this goatnet that was 99.3% of all signal bytes while the mesh
    # it was negotiating carried almost nothing, and it cost peers two thirds of
    # their connections.
    #
    # Attribution is by SIZE, not by a label, because no label exists: the
    # signal body is an opaque NaCl box and the server cannot see the field. A
    # request in the 100 KB..600 KB band on the signal service IS a key-bearing
    # offer — the distribution is strictly bimodal with nothing between ~300 B
    # and ~524 KB, so the band is unambiguous. If that ever stops being true this
    # number becomes wrong, which is why the bucket edges are stated on the page.
    pq = {}
    sig = 'service=~"(nova-)?signal(@file)?"'
    pq["signal_bps"] = scalar(
        prom, f'sum(rate(traefik_service_requests_bytes_total{{{sig}}}[{RI}])) * 8')
    pq["signal_rps"] = scalar(
        prom, f'sum(rate(traefik_service_requests_total{{{sig}}}[{RI}]))')
    # Large-request share of signal BYTES, from the duration histogram's sibling
    # counters. Prometheus has no size histogram for traefik, so this is derived
    # from the mean: bytes/request. A mean is normally a lie on a bimodal
    # distribution, and it is used HERE only to flag presence, never
    # to size the problem.
    pq["mean_request_bytes"] = scalar(
        prom, f'sum(rate(traefik_service_requests_bytes_total{{{sig}}}[{RI}])) '
              f'/ clamp_min(sum(rate(traefik_service_requests_total{{{sig}}}[{RI}])), 0.001)')
    # A mean anywhere near the key size means key-bearing offers dominate.
    # spkSize = 524160; anything above ~10 KB mean is unambiguous, since a real
    # ICE offer is ~260 bytes.
    pq["key_size_bytes"] = 524160

    # HOW MANY PEERS ACTUALLY HAVE IT ON — the only Rosenpass fact that can be
    # measured directly rather than inferred from byte sizes.
    #
    # Everything else in this panel is a PROXY: the signal server emits no
    # byte-size metric, and the protocol forbids labelling the traffic because
    # the message body is an opaque NaCl box the server cannot read. So "how
    # much traffic is Rosenpass" is reachable only by size-band inference, while
    # "how many peers have it enabled" is a column in the management database.
    # Published by rosenpass-compliance-report.py on a 15-minute timer.
    #
    # Shown beside the inference so a reader can tell which numbers are counted
    # and which are derived. They are different kinds of claim and the page
    # should not blur them.
    pq["peers_enabled"] = scalar(prom, 'max(goat_rosenpass_peers_enabled)')
    pq["peers_enabled_connected"] = scalar(
        prom, 'max(goat_rosenpass_peers_enabled_connected)')
    # `owners`, not `owners_affected`. The exporter dropped "affected" because it
    # framed a standing measurement as a remediation; renaming the metric and
    # leaving the consumer on the old name is how a panel goes silently blank
    # while every individual piece still looks correct.
    pq["owners"] = scalar(prom, 'max(goat_rosenpass_owners)')
    pq["peers_known"] = scalar(prom, 'max(goat_rosenpass_peers_known)')
    pq["peers_known_connected"] = scalar(
        prom, 'max(goat_rosenpass_peers_known_connected)')
    # Trend from the SERIES, not from a baseline this file remembers. An earlier
    # version hardcoded "peers at the notice" and published the difference; that
    # is a timestamp pretending to be a fact, correct for a day and then wrong
    # forever on a page nobody re-reads. Prometheus already stores the history —
    # ask it for whatever window is wanted.
    pq["change_24h"] = scalar(prom, 'delta(goat_rosenpass_peers_enabled[24h])')
    pq["change_7d"] = scalar(prom, 'delta(goat_rosenpass_peers_enabled[7d])')
    pq["suspected_pq_offers"] = (
        (pq["signal_bps"] / 8 / 524437.0)
        if pq["signal_bps"] not in (None, 0) else None)
    d["pqc"] = pq

    d["transport"] = tr

    # ── Q4: what is causing it — workload categories + peers ────────────────
    d["workload"] = series(
        prom, f'sum by (category) (rate(goat_net_workload_bytes_total{{{H},{NOT_WORKLOAD},'
              f'direction=~"rx|tx"}}[{RI}])) * 8', "category")
    d["workload_http"] = series(
        prom, f'sum by (category) (rate(goat_http_bytes_total{{{H}}}[{RI}])) * 8', "category")
    d["peers_transport"] = series(
        prom, f'topk(12, sum by (peer, class) (rate(goat_net_peer_bytes_total{{{H}}}[{SOCKET_RI}])) * 8)',
        "peer")
    # the category -> peer drill the brief asked for
    d["peers_by_category"] = series(
        prom, f'topk(25, sum by (category, peer) '
              f'(rate(goat_net_workload_peer_bytes_total{{{H}}}[{SOCKET_RI}])) * 8)', "peer")
    # "which types of interaction" — for the web plane that is the service
    d["services_http"] = series(
        prom, f'topk(12, sum by (service) (rate(goat_http_bytes_total{{{H}}}[{RI}])) * 8)',
        "service")

    # ── Q5: what do I shed ──────────────────────────────────────────────────
    d["shed"] = {
        "policy": [{"key": c["key"], "order": c["shed_order"],
                    "criticality": c["criticality"], "why": c.get("shed_why", "")}
                   for c in shed],
        "essential": essential,
        "reclaimable_bps": scalar(
            prom, f'sum(rate(goat_net_workload_bytes_total{{{H},category=~"{shed_re}"}}[{SOCKET_RI}])) * 8'),
        "protected_bps": scalar(
            prom, f'sum(rate(goat_net_workload_bytes_total{{{H},category=~"{ess_re}"}}[{SOCKET_RI}])) * 8'),
        "reclaimable_by_category": series(
            prom, f'sum by (category) (rate(goat_net_workload_bytes_total{{{H},'
                  f'category=~"{shed_re}"}}[{RI}])) * 8', "category"),
    }
    cap = d["pipe"]["capacity_rx_bps"]
    d["pipe"]["headroom_rx_bps"] = (cap - nic_rx) if (cap is not None and nic_rx is not None) else None

    # ── Q2: is performance degrading ────────────────────────────────────────
    # Streams excluded from percentiles: a stream's lifetime is not a response
    # time. See docs/design/mesh-bandwidth-attribution.md.
    NS = f'service!~"{args_stream}"'
    HAS = ('and on (service) (sum by (service) '
           f'(rate(traefik_service_requests_total[{RI}])) > 0)')
    STRIP = '"service", "$1", "service", "(.*)@file"'
    d["response"] = {
        "p99_by_service": series(
            prom, f'label_replace(histogram_quantile(0.99, sum by (service, le) '
                  f'(rate(traefik_service_request_duration_seconds_bucket{{{NS}}}[{RI}]))) {HAS}, {STRIP})',
            "service"),
        "rps_by_service": series(
            prom, f'label_replace(sum by (service) '
                  f'(rate(traefik_service_requests_total{{{NS}}}[{RI}])) > 0, {STRIP})', "service"),
        "error_ratio": scalar(
            prom, f'sum(rate(traefik_service_requests_total{{{NS},code=~"5.."}}[{RI}])) '
                  f'/ clamp_min(sum(rate(traefik_service_requests_total{{{NS}}}[{RI}])), 1)'),
        "worst_p99": scalar(
            prom, f'max(histogram_quantile(0.99, sum by (service, le) '
                  f'(rate(traefik_service_request_duration_seconds_bucket{{{NS}}}[{RI}]))) {HAS})'),
        # buckets are coarse; say so rather than implying resolution we lack
        "bucket_edges": [0.1, 0.3, 1.2, 5.0],

        # ── THE MEASURED ANSWER, as opposed to the interpolated one ─────────
        # Several services reading the same "0.099 s" is not a default: with
        # every request under le=0.1,
        # histogram_quantile returns 0.1 x 0.99 for ALL of them. Identical
        # numbers that look measured and are arithmetic.
        #
        # The bucket COUNTERS are exact. "what share of requests finished under
        # 100 ms" involves no interpolation at all — it is two counters divided
        # — and it is the thing an operator actually wants to know. A quantile
        # over four coarse buckets is a worse answer to a worse question.
        #
        # Kept alongside p99 rather than replacing it: p99 still separates the
        # genuinely slow (netvis at 1.011 s sits INSIDE a bucket, so its
        # interpolation is meaningful) from the uniformly-fast. The share is
        # what makes the uniformly-fast legible.
        "fast_share_by_service": series(
            prom, f'label_replace('
                  f'sum by (service) (increase(traefik_service_request_duration_seconds_bucket'
                  f'{{{NS},le="0.1"}}[{RI}])) '
                  f'/ clamp_min(sum by (service) (increase('
                  f'traefik_service_request_duration_seconds_count{{{NS}}}[{RI}])), 1) '
                  f'{HAS}, {STRIP})', "service"),
        # Fleet-wide, one number: the headline that replaces a meaningless p99.
        "fast_share_all": scalar(
            prom, f'sum(increase(traefik_service_request_duration_seconds_bucket'
                  f'{{{NS},le="0.1"}}[{RI}])) '
                  f'/ clamp_min(sum(increase('
                  f'traefik_service_request_duration_seconds_count{{{NS}}}[{RI}])), 1)'),
    }

    # streams, measured as streams
    d["streams"] = {
        "services": args_stream.replace("@file", "").split("|"),
        "over_5s_10m": scalar(
            prom, f'sum(increase(traefik_service_request_duration_seconds_count{{service=~"{args_stream}"}}[10m])) '
                  f'- sum(increase(traefik_service_request_duration_seconds_bucket{{service=~"{args_stream}",le="5"}}[10m]))'),
        "open_connections": scalar(prom, 'sum(traefik_entrypoint_open_connections)'),
        "fast_share": series(
            prom, f'label_replace(sum by (service) (increase('
                  f'traefik_service_request_duration_seconds_bucket{{service=~"{args_stream}",le="1.2"}}[30m])) '
                  f'/ clamp_min(sum by (service) (increase('
                  f'traefik_service_request_duration_seconds_count{{service=~"{args_stream}"}}[30m])), 1), {STRIP})',
            "service"),
    }

    # ── path context: p2p vs relayed, measured peers only ───────────────────
    M = 'and on (peer, class) (goat_net_peer_latency_measured == 1)'
    d["path"] = {
        "rtt_p2p_ms": scalar(prom, f'avg(goat_net_peer_latency_ms{{class="mesh-p2p"}} {M})'),
        "rtt_relayed_ms": scalar(prom, f'avg(goat_net_peer_latency_ms{{class="mesh-relayed"}} {M})'),
        "coverage": scalar(
            prom, 'sum(goat_net_peer_latency_measured) / clamp_min(count(goat_net_peer_latency_measured), 1)'),
        "peers_total": scalar(prom, 'count(goat_net_peer_latency_measured)'),
        "peers_relayed": scalar(prom, 'count(goat_net_peer_latency_measured{class="mesh-relayed"})'),
        "slowest": series(prom, f'topk(10, goat_net_peer_latency_ms {M})', "peer"),
        # jitter/loss are NOT here on purpose — the path-quality prober is not
        # deployed on this goatnet, so there are zero series. Absent, not zero.
        "jitter_ms": scalar(prom, 'avg(goat_mesh_rtt_jitter_ms)'),
        "loss_ratio": scalar(prom, 'avg(goat_mesh_packet_loss_ratio)'),
    }

    # ── are the heavy surfaces loading? (ADR 1133) ──────────────────────────
    # Actively fetched, so this answers whether or not anyone is using them —
    # the gap that left panoscope, goat-map, flashlight and netvis watched by
    # nothing at all.
    d["canary"] = canary(prom)

    # ── is the measurement itself trustworthy ───────────────────────────────
    d["trust"] = {
        "attribution_age_s": scalar(prom, f'max(time() - timestamp(goat_net_attribution_up{{{H}}}))'),
        "sample_gap_s": scalar(prom, f'max(goat_net_attribution_sample_gap_seconds{{{H}}})'),
        "mesh_plane_blind": scalar(prom, f'count(goat_net_attribution_mesh_plane_up{{{H}}} == 0)'),
        "peers_sampled": scalar(prom, f'sum(goat_net_attribution_peers_sampled{{{H}}})'),
        "sockets_sampled": scalar(prom, f'sum(goat_net_attribution_sockets_sampled{{{H}}})'),
        "coverage_ratio": scalar(prom, f'max(goat:workload_attribution_coverage:ratio{{{H}}})'),
        "textfile_scrape_error": scalar(prom, 'max(node_textfile_scrape_error)'),
    }
    return d


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--prom", default=DEFAULT_PROM)
    ap.add_argument("--out", required=True)
    ap.add_argument("--host", default=".*", help="host label regex")
    ap.add_argument("--grammar", default="/etc/netview/bandwidth-categories.yaml")
    # COUPLED to the ^nova- metric_relabel_configs block in
    # ops/compose/goat-obs/prometheus/prometheus.yml. That relabel rewrites the
    # `service` label at ingest, unconditionally, on every goatnet running
    # goat-obs -- so a value here carrying the nova- prefix matches NOTHING and
    # the exclusion silently stops working. It does not fail loudly: the
    # streams fall back into the percentile set and worst_p99 reports a
    # held-open SSE lifetime (5.0 s observed) as a response
    # time. Rename the relabel and you must rename this, the role default in
    # roles/netview/defaults/main.yml, and any group_vars override together.
    ap.add_argument("--stream-services", default="signal@file|portal@file",
                    help="services whose duration is a STREAM lifetime, not a "
                         "response time; excluded from percentiles. Must match "
                         "POST-relabel names -- see the comment above.")
    a = ap.parse_args()

    global args_stream
    args_stream = a.stream_services

    grammar = {}
    try:
        import yaml
        with open(a.grammar) as fh:
            grammar = yaml.safe_load(fh) or {}
    except Exception as e:
        print(f"netview-poll: grammar unreadable ({e}) — shed policy will be empty",
              file=sys.stderr)

    doc = build(a.prom, a.host, grammar)

    d = os.path.dirname(a.out) or "."
    os.makedirs(d, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=d, prefix=".netview.")
    with os.fdopen(fd, "w") as f:
        json.dump(doc, f, separators=(",", ":"))
    os.chmod(tmp, 0o644)
    os.replace(tmp, a.out)
    nulls = sum(1 for v in doc["pipe"].values() if v is None)
    print(f"netview-poll: wrote {a.out} ({os.path.getsize(a.out)}b, "
          f"{nulls} unmeasured in pipe)", file=sys.stderr)


if __name__ == "__main__":
    main()
