# SPDX-License-Identifier: Apache-2.0
"""A received feed's datagrams, byte for byte, onto the fabric.

A passive copy: it reads packets from the network interface and never opens the
feed's port, so the converter that owns the port is not affected. One sample is
one UDP payload, unchanged. The attachment carries the sender and the receive
time. Runs as its own process (`python -m goatmap.rawtap`) with CAP_NET_RAW.

Environment: RAWTAP_IFACE, RAWTAP_LISTEN_IP, RAWTAP_PORTS=port:feed[,port:feed],
RAWTAP_PREFIX (a volatile/ key prefix), RAWTAP_ZENOH_CONFIG.
"""

from __future__ import annotations

import json
import logging
import os
import socket
import struct
import time

LOG = logging.getLogger("goatmap.rawtap")


def parse(pkt: bytes, dst: bytes, ports: dict[int, str]) -> tuple[str, str, bytes] | None:
    """(feed, sender, payload) of an unfragmented IPv4 UDP packet to one of the
    ports, else None."""
    if len(pkt) < 28 or pkt[0] >> 4 != 4 or pkt[9] != 17 or pkt[16:20] != dst:
        return None
    if struct.unpack(">H", pkt[6:8])[0] & 0x3FFF:
        return None
    h = (pkt[0] & 15) * 4
    total = struct.unpack(">H", pkt[2:4])[0]
    if len(pkt) < total or total < h + 8:
        return None
    sport, dport, length = struct.unpack(">HHH", pkt[h:h + 6])
    feed = ports.get(dport)
    if feed is None or length < 8 or h + length > total:
        return None
    return feed, f"{socket.inet_ntoa(pkt[12:16])}:{sport}", bytes(pkt[h + 8:h + length])


def main() -> None:
    import zenoh

    ports = {int(p): f for p, f in (x.split(":", 1) for x in os.environ["RAWTAP_PORTS"].split(","))}
    dst = socket.inet_aton(os.environ["RAWTAP_LISTEN_IP"])
    prefix = os.environ["RAWTAP_PREFIX"].rstrip("/")
    sock = socket.socket(socket.AF_PACKET, socket.SOCK_DGRAM, socket.htons(0x0800))
    sock.bind((os.environ["RAWTAP_IFACE"], 0x0800))
    session = zenoh.open(zenoh.Config.from_file(os.environ["RAWTAP_ZENOH_CONFIG"]))
    pubs = {f: session.declare_publisher(f"{prefix}/{f}", congestion_control=zenoh.CongestionControl.DROP)
            for f in ports.values()}
    n, said = 0, 0.0
    while True:
        pkt, addr = sock.recvfrom(65535)
        if addr[2] == 4:  # PACKET_OUTGOING
            continue
        got = parse(pkt, dst, ports)
        if got is None:
            continue
        feed, sender, payload = got
        pubs[feed].put(payload, attachment=json.dumps({"from": sender, "recv_ms": int(time.time() * 1000)}).encode())
        n += 1
        if time.monotonic() - said > 300:
            said = time.monotonic()
            LOG.info("published %d datagrams", n)


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    main()
