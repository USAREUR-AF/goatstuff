//! The whole path over a real Zenoh transport, with no router and no network
//! beyond the loopback interface: one in-process peer stands in for the
//! fabric, the shim connects to it as a client, and a message published in
//! one format arrives converted under the declared key.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use goat_transformers::Distribution;
use transform_shim::{announce_key, health_key, load_bundle, run, Config, RunOptions, Table};

const COT: &str = r#"<event version="2.0" uid="4411" type="a-f-A" time="2021-02-27T20:00:00.250Z" how="m-g"><point lat="30.25" lon="-85.5" hae="120.5" ce="25.5" le="40"/></event>"#;

async fn session(mode: &str, listen: Option<&str>, connect: Option<&str>) -> zenoh::Session {
    let mut c = zenoh::Config::default();
    c.insert_json5("mode", &format!("\"{mode}\"")).unwrap();
    c.insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    if let Some(l) = listen {
        c.insert_json5("listen/endpoints", &format!("[\"{l}\"]"))
            .unwrap();
    }
    if let Some(e) = connect {
        c.insert_json5("connect/endpoints", &format!("[\"{e}\"]"))
            .unwrap();
    }
    zenoh::open(c).await.unwrap()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_message_is_converted_and_republished_and_a_bad_one_is_reported() {
    let endpoint = format!("tcp/127.0.0.1:{}", free_port());
    let fabric = session("peer", Some(&endpoint), None).await;

    let cfg = Config::from_json(
        r#"{"instance":"t1","site":"zagros","routes":[{"name":"cot","subscribe":"telemetry/cot/zagros/*",
            "from":"cot-xml","to":"geojson","strip_prefix":"telemetry/cot/","publish_prefix":"data/entity/observation/"},
            {"name":"geo","subscribe":"telemetry/sensor/*/cot/*","from":"cot-xml","to":"geo-track-v1",
            "strip_prefix":"telemetry/sensor/","publish_prefix":"data/geo/track/v1/",
            "set":{"geotrack.kind":"{key.-1}","geotrack.sensor":"{capture.1}","geotrack.site":"zagros","geotrack.node":"t1",
                   "geotrack.observed_at":"{received_at}","geotrack.producer_anchor":"00112233445566778899aabbccddeeff",
                   "geotrack.synthetic":"0","geotrack.org":"{attachment.org}"}}]}"#,
    )
    .unwrap();
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("bundle");
    let table =
        Arc::new(Table::build(&cfg, load_bundle(&bundle, Distribution::Public).unwrap()).unwrap());

    let converted = fabric
        .declare_subscriber("data/entity/observation/**")
        .await
        .unwrap();
    let announce = fabric.declare_subscriber(announce_key(&cfg)).await.unwrap();
    let health = fabric.declare_subscriber(health_key(&cfg)).await.unwrap();

    let shim_session = session("client", None, Some(&endpoint)).await;
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let shim = tokio::spawn(run(
        shim_session,
        cfg.clone(),
        table.clone(),
        RunOptions {
            queue_depth: 16,
            heartbeat: Duration::from_millis(300),
            config_digest: "test".into(),
        },
        async {
            let _ = stop_rx.await;
        },
    ));

    let wait = Duration::from_secs(10);
    let a = tokio::time::timeout(wait, announce.recv_async())
        .await
        .expect("announcement")
        .unwrap();
    let a: serde_json::Value = serde_json::from_slice(&a.payload().to_bytes()).unwrap();
    assert_eq!(a["class"], "transform");
    assert_eq!(a["translates_from"][0], "cot-xml");
    assert_eq!(
        a["translates_to"],
        serde_json::json!(["geo-track-v1", "geojson"])
    );

    // The shim's subscription has to reach this peer before a publish finds it.
    let mut got = None;
    for _ in 0..50 {
        fabric.put("telemetry/cot/zagros/4411", COT).await.unwrap();
        if let Ok(Ok(s)) =
            tokio::time::timeout(Duration::from_millis(200), converted.recv_async()).await
        {
            got = Some(s);
            break;
        }
    }
    let sample = got.expect("a converted message");
    assert_eq!(
        sample.key_expr().as_str(),
        "data/entity/observation/zagros/4411"
    );
    assert_eq!(
        sample.encoding().to_string(),
        "application/geo+json;geojson"
    );
    let v: serde_json::Value = serde_json::from_slice(&sample.payload().to_bytes()).unwrap();
    assert_eq!(v["id"], "4411");
    assert_eq!(v["geometry"]["coordinates"][1], 30.25);

    // A route whose context comes from the arrival: the key, the receive
    // clock and the sample's attachment reach the converted track.
    let tracks = fabric
        .declare_subscriber("data/geo/track/v1/**")
        .await
        .unwrap();
    let before = transform_shim::now_ms();
    fabric
        .put("telemetry/sensor/radar-a/cot/alert", COT)
        .attachment(r#"{"org":"Example Org"}"#)
        .await
        .unwrap();
    let track = tokio::time::timeout(wait, tracks.recv_async())
        .await
        .expect("a converted track")
        .unwrap();
    assert_eq!(
        track.key_expr().as_str(),
        "data/geo/track/v1/radar-a/cot/alert"
    );
    assert_eq!(
        track.encoding().to_string(),
        "application/json;geo-track-v1"
    );
    let v: serde_json::Value = serde_json::from_slice(&track.payload().to_bytes()).unwrap();
    assert_eq!(v["id"], "radar-a:4411");
    assert_eq!(v["kind"], "alert");
    assert_eq!(v["source"]["sensor"], "radar-a");
    assert_eq!(v["org"], "Example Org");
    let observed = v["observed_at"].as_str().unwrap();
    assert!(
        observed >= transform_shim::iso8601_ms(before).as_str()
            && observed <= transform_shim::iso8601_ms(transform_shim::now_ms()).as_str(),
        "{observed}"
    );

    // A message the library refuses produces no output and shows in the heartbeat.
    fabric
        .put("telemetry/cot/zagros/bad", "<not-xml")
        .await
        .unwrap();
    let mut reported = false;
    for _ in 0..40 {
        let h = tokio::time::timeout(wait, health.recv_async())
            .await
            .expect("heartbeat")
            .unwrap();
        let h: serde_json::Value = serde_json::from_slice(&h.payload().to_bytes()).unwrap();
        if h["routes"][0]["refused"] == 1 {
            assert!(h["routes"][0]["converted"].as_u64().unwrap() >= 1);
            // The library's reason, as it gave it. The Rust engine does not
            // prefix an XML syntax error with an error kind.
            assert!(!h["routes"][0]["last_error"].as_str().unwrap().is_empty());
            assert_eq!(h["state"], "degraded");
            reported = true;
            break;
        }
    }
    assert!(reported, "the refusal did not appear in the heartbeat");
    // One message in, at most one out: nothing was published for the bad one.
    while let Ok(Ok(s)) =
        tokio::time::timeout(Duration::from_millis(300), converted.recv_async()).await
    {
        assert_eq!(s.key_expr().as_str(), "data/entity/observation/zagros/4411");
    }

    stop_tx.send(()).unwrap();
    shim.await.unwrap().unwrap();
}
