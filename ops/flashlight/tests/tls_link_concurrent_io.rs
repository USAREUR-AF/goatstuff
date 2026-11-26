// SPDX-License-Identifier: Apache-2.0
//
// Regression test: one zenoh TLS link must survive a read and a write running
// at the same time.
//
// The published zenoh-link-tls 1.x drives one TLS stream from its read task
// and its write task at the same moment. rustls 0.23.46 and later check
// connection state on every write, and with that sharing writes fail with
// "handshake not complete" under inbound load, which tears the link down.
// This workspace takes zenoh-link-tls from the patched copy in
// vendor/zenoh-link-tls ([patch.crates-io] in Cargo.toml; finding F-614).
//
// The test opens a real TLS link with the same link manager zenoh uses and
// runs that shape: the server floods the client while the client writes small
// frames and reads concurrently. Against the published crate with rustls
// 0.23.46 the client's writes fail within the run; against the patched copy
// every write succeeds.

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use zenoh_link_commons::{LinkManagerUnicastTrait, LinkUnicast, LinkUnicastTrait, NewLink};
use zenoh_link_tls::LinkManagerUnicastTls;
use zenoh_protocol::core::EndPoint;

/// How long both directions run concurrently.
const RUN_FOR: Duration = Duration::from_secs(4);
/// Bytes per server→client write; large enough that the client's read path
/// is inside rustls processing records for most of the run.
const FLOOD_CHUNK: usize = 16 * 1024;
/// Bytes per client→server write; the size of a small zenoh frame.
const CLIENT_FRAME: usize = 64;
/// Longest a single client write may take before the test calls it stalled.
const WRITE_STALL: Duration = Duration::from_secs(5);
/// Bound on listener start, TLS handshake and accept.
const SETUP: Duration = Duration::from_secs(20);

struct Pki {
    ca: String,
    server_cert: String,
    server_key: String,
}

fn write_pki(dir: &Path) -> Pki {
    let ca_key = KeyPair::generate().expect("generate CA key");
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "test root");
    let ca_cert = ca_params.self_signed(&ca_key).expect("self-sign CA");
    let issuer = Issuer::new(ca_params, ca_key);

    let server_key = KeyPair::generate().expect("generate server key");
    let mut server_params =
        CertificateParams::new(vec!["localhost".to_string()]).expect("server params");
    server_params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    let server_cert = server_params
        .signed_by(&server_key, &issuer)
        .expect("sign server cert");

    let pki = Pki {
        ca: dir.join("ca.pem").display().to_string(),
        server_cert: dir.join("server.pem").display().to_string(),
        server_key: dir.join("server.key").display().to_string(),
    };
    std::fs::write(&pki.ca, ca_cert.pem()).unwrap();
    std::fs::write(&pki.server_cert, server_cert.pem()).unwrap();
    std::fs::write(&pki.server_key, server_key.serialize_pem()).unwrap();
    pki
}

fn single(link: LinkUnicast) -> Arc<dyn LinkUnicastTrait> {
    match link.0 {
        NewLink::Single(l) => l,
        NewLink::MixedReliability { reliable, .. } => reliable,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_link_survives_concurrent_read_and_write() {
    let dir = std::env::temp_dir().join(format!("flashlight-tls-link-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pki = write_pki(&dir);

    // Server side: a listener on an ephemeral port; accepted links arrive on
    // the channel exactly as they do inside a zenoh router.
    let (accepted_tx, accepted_rx) = flume::unbounded();
    let server = LinkManagerUnicastTls::new(accepted_tx);
    let listen: EndPoint = format!(
        "tls/127.0.0.1:0#root_ca_certificate_file={};listen_certificate_file={};listen_private_key_file={}",
        pki.ca, pki.server_cert, pki.server_key
    )
    .parse()
    .expect("listen endpoint");
    let locator = tokio::time::timeout(SETUP, server.new_listener(listen))
        .await
        .expect("listener up within SETUP")
        .expect("listen");
    let port = locator
        .address()
        .as_str()
        .rsplit(':')
        .next()
        .unwrap()
        .to_string();

    // Client side: the same manager flashlight's zenoh session uses.
    let (unused_tx, _unused_rx) = flume::unbounded();
    let client = LinkManagerUnicastTls::new(unused_tx);
    let connect: EndPoint = format!("tls/localhost:{port}#root_ca_certificate_file={}", pki.ca)
        .parse()
        .expect("connect endpoint");
    let client_link = single(
        tokio::time::timeout(SETUP, client.new_link(connect))
            .await
            .expect("TLS handshake within SETUP")
            .expect("connect"),
    );
    let server_link = single(
        tokio::time::timeout(SETUP, accepted_rx.recv_async())
            .await
            .expect("server accepted within SETUP")
            .expect("accepted link"),
    );

    let stop = Arc::new(AtomicBool::new(false));

    // Server floods the client, so the client's read path is busy in rustls.
    // It keeps flooding until aborted, so the client reader's last read
    // always completes and the reader can observe `stop`.
    let flood = {
        let link = server_link.clone();
        tokio::spawn(async move {
            let chunk = vec![0xA5u8; FLOOD_CHUNK];
            while link.write_all(&chunk, None).await.is_ok() {}
        })
    };
    // Server drains what the client writes, so client writes never stall.
    let drain = {
        let link = server_link.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n) = link.read(&mut buf, None).await {
                if n == 0 {
                    break;
                }
            }
        })
    };
    // Client reads continuously — zenoh's RX task.
    let client_reader = {
        let link = client_link.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024];
            let mut total = 0usize;
            while !stop.load(Ordering::Relaxed) {
                match link.read(&mut buf, None).await {
                    Ok(0) => break,
                    Ok(n) => total += n,
                    Err(e) => return Err(format!("client read failed after {total} bytes: {e}")),
                }
            }
            Ok(total)
        })
    };

    // Client writes small frames concurrently — zenoh's TX task.
    let started = Instant::now();
    let frame = vec![0x5Au8; CLIENT_FRAME];
    let mut writes = 0u64;
    let mut write_error = None;
    while started.elapsed() < RUN_FOR {
        // A bounded wait: a write that never completes (a lost wakeup in the
        // link) fails the test instead of hanging it.
        match tokio::time::timeout(WRITE_STALL, client_link.write_all(&frame, None)).await {
            Ok(Ok(())) => writes += 1,
            Ok(Err(e)) => {
                write_error = Some(format!("client write #{} failed: {e}", writes + 1));
                break;
            }
            Err(_) => {
                write_error = Some(format!(
                    "client write #{} stalled for {WRITE_STALL:?}",
                    writes + 1
                ));
                break;
            }
        }
        // Yield now and then so the reader and the flood interleave with the
        // writer on every worker, as they do in a live session.
        if writes % 64 == 0 {
            tokio::task::yield_now().await;
        }
    }

    stop.store(true, Ordering::Relaxed);
    eprintln!("run finished: {writes} client writes in {:?}", started.elapsed());
    if let Some(e) = write_error {
        client_reader.abort();
        flood.abort();
        drain.abort();
        let _ = std::fs::remove_dir_all(&dir);
        panic!("{e} — concurrent read and write broke the TLS link");
    }
    let read_result = tokio::time::timeout(Duration::from_secs(10), client_reader)
        .await
        .expect("client reader stops within 10s")
        .expect("client reader task");
    // Teardown is not under test. A graceful `close` flushes what the
    // aborted flood left buffered towards a client that no longer reads, so
    // it can wait indefinitely; bound it and move on.
    flood.abort();
    drain.abort();
    let _ = flood.await;
    let _ = drain.await;
    let _ = tokio::time::timeout(Duration::from_secs(2), client_link.close()).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), server_link.close()).await;
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("teardown finished");

    let read_bytes = read_result.expect("client reads succeed while writing");
    assert!(writes > 1000, "too few writes to exercise the race: {writes}");
    assert!(
        read_bytes > FLOOD_CHUNK * 100,
        "too little inbound data to exercise the race: {read_bytes} bytes"
    );
}
