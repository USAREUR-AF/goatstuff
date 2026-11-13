// SPDX-License-Identifier: Apache-2.0
//
// End to end without a router: a recorded router log (made-up Zenoh IDs, ANSI
// colour on some lines, the WARN/ERROR pair per rejection, a short ID, a
// garbage line) goes through the file follower, the detector and the publisher
// trait; the test decodes what would have been put on the fabric.

use goat_peer_health::clock::LocalClock;
use goat_peer_health::detector::{Config, Detector};
use goat_peer_health::follow::FileFollower;
use goat_peer_health::wire::{EnvDetail, EnvHealthV1};
use goat_peer_health::{publish_reading, Attribution, Publisher, VerdictDoc};
use prost::Message as _;
use std::future::Future;

const FAST: &str = "3c9a51e0d2b7f4a6c18e05d9b3a7f2e1";
const SHORT: &str = "9f2";
const ROUTER: &str = "5be08d2f41a97c36e0d1f8b2a4c6e9d7";
/// 2026-11-13T13:18:30Z, the first line of the fixture.
const T0: i64 = 1_794_575_910_000;

#[derive(Default)]
struct Capture(Vec<(String, Vec<u8>)>);

impl Publisher for Capture {
    fn put(
        &mut self,
        key: &str,
        payload: Vec<u8>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send {
        self.0.push((key.to_string(), payload));
        std::future::ready(Ok(()))
    }
}

fn envelope(c: &Capture) -> EnvHealthV1 {
    let (_, bytes) =
        c.0.iter()
            .find(|(k, _)| k == "ops/peer/health/example-site")
            .expect("an envelope on ops/peer/health/example-site");
    EnvHealthV1::decode(bytes.as_slice()).expect("decodes")
}

fn verdict(c: &Capture, zid: &str) -> Option<VerdictDoc> {
    let key = format!("ops/peer/verdict/{zid}");
    c.0.iter()
        .find(|(k, _)| *k == key)
        .map(|(_, b)| serde_json::from_slice(b).expect("verdict JSON"))
}

#[tokio::test]
async fn a_recorded_log_produces_the_documents() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/router.log");
    let mut follower = FileFollower::new(fixture, true);
    let lines = follower.poll();
    assert_eq!(lines.len(), 18, "every fixture line is delivered");

    let attribution = Attribution::from_json(&format!(
        r#"{{"{FAST}":{{"name":"lab-router-b","owner":"example partner"}}}}"#
    ))
    .unwrap();
    let cfg = Config {
        site: "example-site".to_string(),
        peer_population: 5,
        ..Config::default()
    };
    let mut det = Detector::new(cfg, attribution);
    det.set_sessions(Some(vec![SHORT.to_string()]));
    let arrival = T0 + 30_000;
    for l in &lines {
        det.ingest(l, arrival);
    }

    // First tick, ten seconds after the last line.
    let reading = det.tick(T0 + 40_000, &LocalClock::Synced);
    let mut cap = Capture::default();
    publish_reading(&mut cap, "example-site", &reading)
        .await
        .unwrap();

    let env = envelope(&cap);
    assert_eq!(env.domain, "peer");
    assert_eq!(env.scope, "example-site");
    assert_eq!(env.overall_color, "red");
    assert_eq!(env.window_secs, 60);
    let Some(EnvDetail::Peer(d)) = env.detail else {
        panic!("peer detail")
    };
    assert_eq!(d.verdict, "peer_at_fault");
    assert_eq!(d.observed_by, ROUTER);
    assert_eq!(d.local_clock, "synced");
    assert_eq!(d.peers_known, 5);
    assert_eq!(d.unparsed_lines_window, 0);
    assert_eq!(d.peers.len(), 2);

    let fast = d.peers.iter().find(|p| p.zid == FAST).unwrap();
    // Six WARN lines; the six ERROR lines are not counted.
    assert_eq!(fast.rejections_in_window, 6);
    assert_eq!(fast.rejections_per_minute, 6.0);
    assert!(
        (fast.offset_seconds - 17.705).abs() < 1e-6,
        "{}",
        fast.offset_seconds
    );
    assert!((fast.min_offset_seconds - 17.700).abs() < 1e-6);
    assert!((fast.max_offset_seconds - 17.705).abs() < 1e-6);
    assert_eq!(fast.limit_seconds, 0.5);
    assert_eq!(fast.first_seen_unix_ms, T0 + 5_152);
    assert_eq!(fast.last_seen_unix_ms, T0 + 25_152);
    assert_eq!(fast.attribution_name.as_deref(), Some("lab-router-b"));
    assert_eq!(fast.attribution_owner.as_deref(), Some("example partner"));
    assert_eq!(fast.observed_by, ROUTER);
    assert_eq!(fast.direct_session, Some(false));
    assert_eq!(fast.grade, "peer_at_fault");

    let short = d.peers.iter().find(|p| p.zid == SHORT).unwrap();
    assert_eq!(short.rejections_in_window, 2);
    assert_eq!(short.offset_seconds, 3.0);
    assert_eq!(short.attribution_name, None);
    assert_eq!(short.direct_session, Some(true));

    let v = verdict(&cap, FAST).expect("a verdict for the fast peer");
    assert_eq!(v.state, "at_fault");
    assert_eq!(v.fault, "clock_ahead");
    assert_eq!(v.limit_seconds, 0.5);
    assert_eq!(v.observed_by, ROUTER);
    assert_eq!(v.observed_at, "2026-11-13T13:19:10.000Z");
    assert_eq!(
        v.sentence,
        "the router sees your clock 17.7 s ahead of its own; the limit is 0.5 s"
    );
    assert!(verdict(&cap, SHORT).is_some());
    assert_eq!(cap.0.len(), 3, "one envelope and two verdicts");

    // The same history with our own clock unsynchronized: nobody is blamed.
    let reading = det.tick(T0 + 41_000, &LocalClock::Unsynced("test".into()));
    let mut cap = Capture::default();
    publish_reading(&mut cap, "example-site", &reading)
        .await
        .unwrap();
    assert_eq!(cap.0.len(), 1, "envelope only");
    assert_eq!(envelope(&cap).overall_color, "amber");

    // Eleven minutes later, with the log still alive: both cleared, once.
    det.ingest("INFO keepalive", T0 + 690_000);
    let reading = det.tick(T0 + 690_000, &LocalClock::Synced);
    let mut cap = Capture::default();
    publish_reading(&mut cap, "example-site", &reading)
        .await
        .unwrap();
    let Some(EnvDetail::Peer(d)) = envelope(&cap).detail else {
        panic!("peer detail")
    };
    assert!(d.peers.iter().all(|p| p.grade == "cleared"));
    assert_eq!(verdict(&cap, FAST).unwrap().state, "cleared");
    let reading = det.tick(T0 + 720_000, &LocalClock::Synced);
    assert!(reading.verdicts.is_empty());
    assert_eq!(reading.detail().verdict, "clean");
}
