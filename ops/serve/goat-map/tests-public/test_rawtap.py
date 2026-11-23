# SPDX-License-Identifier: Apache-2.0
import socket
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from goatmap.rawtap import parse  # noqa: E402

DST = socket.inet_aton("198.51.100.9")
PORTS = {7100: "feed-a"}


def packet(body=b"\xfd\x01\x00raw", dport=7100, dst=DST, proto=17, frag=0, pad=b""):
    n = 28 + len(body)
    return (struct.pack(">BBHHHBBH", 0x45, 0, n, 1, frag, 64, proto, 0) + socket.inet_aton("198.51.100.1") + dst
            + struct.pack(">HHHH", 5000, dport, 8 + len(body), 0) + body + pad)


def test_the_payload_is_unchanged_and_padding_is_not_part_of_it():
    assert parse(packet(pad=b"\0" * 6), DST, PORTS) == ("feed-a", "198.51.100.1:5000", b"\xfd\x01\x00raw")


def test_other_traffic_is_ignored():
    for p in (packet(dport=53), packet(dst=b"\1\2\3\4"), packet(proto=6), packet(frag=0x2000), packet()[:30], b""):
        assert parse(p, DST, PORTS) is None
