//! Private mesh DNS — the addressing layer that carries NO trust.
//!
//! `resolve` does a REAL A-record query against the sandbox CoreDNS (proving
//! the name resolves to an address and nothing more). `repoint` rewrites the
//! zone file IN PLACE (same inode, so the bind-mounted CoreDNS sees it and
//! `reload` picks it up) and bumps the serial — the burn-&-respawn mechanic
//! where the stable name re-points to a fresh endpoint while the name is
//! constant. Trust is re-earned per connection regardless of what the name says.

use anyhow::{anyhow, Context, Result};
use std::net::{ToSocketAddrs, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Resolve an A record via the given DNS server ("coredns:53"). Hand-rolled to
/// avoid a heavy resolver dep; one A question, parse A answers.
pub fn resolve(server: &str, name: &str) -> Result<Vec<String>> {
    let addr = server
        .to_socket_addrs()
        .context("resolve dns server addr")?
        .next()
        .ok_or_else(|| anyhow!("no addr for {server}"))?;

    let sock = UdpSocket::bind("0.0.0.0:0").context("bind udp")?;
    sock.set_read_timeout(Some(Duration::from_secs(3)))?;
    sock.connect(addr)?;

    let mut q = Vec::new();
    q.extend_from_slice(&[0x12, 0x34]); // id
    q.extend_from_slice(&[0x01, 0x00]); // flags: RD
    q.extend_from_slice(&[0x00, 0x01]); // qdcount
    q.extend_from_slice(&[0x00, 0x00]); // ancount
    q.extend_from_slice(&[0x00, 0x00]); // nscount
    q.extend_from_slice(&[0x00, 0x00]); // arcount
    for label in name.trim_end_matches('.').split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0x00); // root
    q.extend_from_slice(&[0x00, 0x01]); // qtype A
    q.extend_from_slice(&[0x00, 0x01]); // qclass IN

    sock.send(&q)?;
    let mut buf = [0u8; 1500];
    let n = sock.recv(&mut buf)?;
    parse_a_answers(&buf[..n])
}

fn parse_a_answers(msg: &[u8]) -> Result<Vec<String>> {
    if msg.len() < 12 {
        return Err(anyhow!("short dns response"));
    }
    let qd = u16::from_be_bytes([msg[4], msg[5]]) as usize;
    let an = u16::from_be_bytes([msg[6], msg[7]]) as usize;
    let mut pos = 12;
    // skip questions
    for _ in 0..qd {
        pos = skip_name(msg, pos)?;
        pos += 4; // qtype + qclass
    }
    let mut out = Vec::new();
    for _ in 0..an {
        pos = skip_name(msg, pos)?;
        if pos + 10 > msg.len() {
            break;
        }
        let rtype = u16::from_be_bytes([msg[pos], msg[pos + 1]]);
        let rdlen = u16::from_be_bytes([msg[pos + 8], msg[pos + 9]]) as usize;
        pos += 10;
        if rtype == 1 && rdlen == 4 && pos + 4 <= msg.len() {
            out.push(format!(
                "{}.{}.{}.{}",
                msg[pos], msg[pos + 1], msg[pos + 2], msg[pos + 3]
            ));
        }
        pos += rdlen;
    }
    Ok(out)
}

/// Skip a (possibly compressed) DNS name, returning the offset past it.
fn skip_name(msg: &[u8], mut pos: usize) -> Result<usize> {
    loop {
        if pos >= msg.len() {
            return Err(anyhow!("name overrun"));
        }
        let len = msg[pos];
        if len & 0xC0 == 0xC0 {
            return Ok(pos + 2); // pointer ends the name
        }
        if len == 0 {
            return Ok(pos + 1);
        }
        pos += 1 + len as usize;
    }
}

/// Rewrite the zone so `sensor-gw` resolves to `ip`, bumping the serial. Written
/// in place (same path → same inode → bind-mounted CoreDNS reloads it).
pub fn repoint(zone_file: &str, ip: &str) -> Result<u32> {
    let serial = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(1);
    let zone = format!(
        "$ORIGIN trust.local.\n\
$TTL 5\n\
@   IN  SOA ns.trust.local. admin.trust.local. (\n\
        {serial} ; serial (orchestrator bumps on burn-&-respawn)\n\
        7200 3600 1209600 5 )\n\
@   IN  NS  ns.trust.local.\n\
ns  IN  A   127.0.0.1\n\
sensor-gw   IN  A   {ip}\n"
    );
    std::fs::write(zone_file, zone).context("rewrite zone")?;
    Ok(serial)
}
