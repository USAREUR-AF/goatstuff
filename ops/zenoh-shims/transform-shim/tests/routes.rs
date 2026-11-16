//! The convert step and the start-up checks, with no fabric session.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use goat_transformers::Distribution;
use sha2::{Digest, Sha256};
use transform_shim::{captures, iso8601_ms, load_bundle, Arrival, Config, Table, Template};

const COT: &str = r#"<event version="2.0" uid="4411" type="a-f-A" time="2021-02-27T20:00:00.250Z" start="2021-02-27T20:00:00.250Z" stale="2021-02-27T20:03:00.250Z" how="m-g"><point lat="30.25" lon="-85.5" hae="120.5" ce="25.5" le="40"/><detail><track course="90" speed="12.5"/></detail></event>"#;

fn bundle() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("bundle")
}

fn table(routes: &str) -> anyhow::Result<Table> {
    let cfg = Config::from_json(&format!(
        r#"{{"instance":"t","site":"zagros","routes":{routes}}}"#
    ))?;
    Table::build(&cfg, load_bundle(&bundle(), Distribution::Public)?)
}

fn route(name: &str, sub: &str, from: &str, to: &str, strip: &str, publish: &str) -> String {
    format!(
        r#"{{"name":"{name}","subscribe":"{sub}","from":"{from}","to":"{to}","strip_prefix":"{strip}","publish_prefix":"{publish}"}}"#
    )
}

fn refused(routes: &str, needle: &str) {
    let err = match table(routes) {
        Ok(_) => panic!("the table was accepted: {routes}"),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains(needle), "expected `{needle}` in `{err}`");
}

#[test]
fn text_route_converts_and_rewrites_the_key() {
    let r = route(
        "a",
        "telemetry/cot/zagros/*",
        "cot-xml",
        "geojson",
        "telemetry/cot/",
        "data/entity/observation/",
    );
    let t = table(&format!("[{r}]")).unwrap();
    let out = t
        .convert(0, "telemetry/cot/zagros/4411", COT.as_bytes())
        .unwrap();
    assert_eq!(out.key, "data/entity/observation/zagros/4411");
    assert_eq!(out.encoding, "application/geo+json;geojson");
    assert_eq!(
        String::from_utf8(out.payload).unwrap(),
        r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[-85.5,30.25,120.5]},"properties":{"type":"a-f-A","time_ms":1614456000250,"how":"m-g","ce":25.5,"le":40,"course":90,"speed_ms":12.5},"id":"4411"}"#
    );
    assert_eq!(t.routes[0].counters.converted.load(Ordering::Relaxed), 1);
}

#[test]
fn binary_route_carries_bytes_not_hex_text() {
    let to_pb = route(
        "pb",
        "telemetry/cot/zagros/*",
        "cot-xml",
        "cot-protobuf",
        "telemetry/cot/",
        "telemetry/takproto/",
    );
    let t = table(&format!("[{to_pb}]")).unwrap();
    let pb = t
        .convert(0, "telemetry/cot/zagros/4411", COT.as_bytes())
        .unwrap();
    assert_eq!(pb.encoding, "application/protobuf;cot-protobuf");
    // A protobuf message, not its hex text: the uid appears as bytes.
    assert!(pb.payload.windows(4).any(|w| w == b"4411"));
    assert!(std::str::from_utf8(&pb.payload)
        .map(|s| !s.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or(true));

    let back = route(
        "xml",
        "telemetry/takproto/zagros/*",
        "cot-protobuf",
        "cot-xml",
        "telemetry/takproto/",
        "telemetry/cot2/",
    );
    let t = table(&format!("[{back}]")).unwrap();
    let xml = t.convert(0, &pb.key, &pb.payload).unwrap();
    let xml = String::from_utf8(xml.payload).unwrap();
    assert!(
        xml.contains(r#"uid="4411""#) && xml.contains(r#"lat="30.25""#),
        "{xml}"
    );
}

#[test]
fn context_reaches_the_conversion() {
    let r = r#"{"name":"g","subscribe":"telemetry/cot/zagros/*","from":"cot-xml","to":"geo-track-v1",
        "strip_prefix":"telemetry/cot/","publish_prefix":"data/entity/track/",
        "set":{"geotrack.kind":"track","geotrack.sensor":"radar-a","geotrack.site":"site-a","geotrack.node":"node-a",
               "geotrack.observed_at":"2021-02-27T20:00:01.000Z","geotrack.producer_anchor":"00112233445566778899aabbccddeeff",
               "geotrack.synthetic":"1"}}"#;
    let t = table(&format!("[{r}]")).unwrap();
    let out = t
        .convert(0, "telemetry/cot/zagros/4411", COT.as_bytes())
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.payload).unwrap();
    assert_eq!(v["id"], "radar-a:4411");
    assert_eq!(v["synthetic"], true);
    assert_eq!(v["schema"], "goat.geo.track.v1");
}

#[test]
fn a_refused_message_is_counted_and_keeps_its_reason() {
    let r = route(
        "a",
        "telemetry/cot/zagros/*",
        "cot-xml",
        "geojson",
        "telemetry/cot/",
        "data/entity/observation/",
    );
    let t = table(&format!("[{r}]")).unwrap();
    assert!(t.convert(0, "telemetry/cot/zagros/1", b"<not-xml").is_err());
    assert!(t
        .convert(0, "telemetry/cot/zagros/1", &[0xff, 0xfe])
        .is_err());
    let c = &t.routes[0].counters;
    assert_eq!(c.refused.load(Ordering::Relaxed), 2);
    assert_eq!(c.converted.load(Ordering::Relaxed), 0);
    assert!(c
        .last_error
        .lock()
        .unwrap()
        .as_deref()
        .unwrap()
        .contains("malformed-wire"));
}

fn arrival(key: &str) -> Arrival {
    Arrival {
        key: key.into(),
        received_ms: 1_614_456_001_250,
        encoding: "application/xml".into(),
        attachment: Some(br#"{"org":"Example Org","hop":3,"live":true,"nested":{}}"#.to_vec()),
        timestamp_ms: Some(1_614_456_000_900),
    }
}

#[test]
fn a_set_value_takes_what_the_arrival_knows() {
    let sub = "telemetry/sensor/*/cot/**";
    let a = arrival("telemetry/sensor/radar-a/cot/track/4411");
    let render = |text: &str| {
        Template::parse(text, 2)
            .unwrap()
            .render(&a, sub, "telemetry/sensor/")
    };
    for (text, want) in [
        ("fixed text", "fixed text"),
        ("{received_ms}", "1614456001250"),
        ("{received_at}", "2021-02-27T20:00:01.250Z"),
        ("{timestamp_ms}", "1614456000900"),
        ("{timestamp_at}", "2021-02-27T20:00:00.900Z"),
        ("{key}", "telemetry/sensor/radar-a/cot/track/4411"),
        ("{key.0}", "telemetry"),
        ("{key.2}", "radar-a"),
        ("{key.-1}", "4411"),
        ("{key.-2}", "track"),
        ("{capture.1}", "radar-a"),
        ("{capture.2}", "track/4411"),
        ("{tail}", "radar-a/cot/track/4411"),
        ("{encoding}", "application/xml"),
        ("{attachment.org}", "Example Org"),
        ("{attachment.hop}", "3"),
        ("{attachment.live}", "true"),
        ("{capture.1}:{key.-1}", "radar-a:4411"),
        ("a {{brace}} and {key.0}", "a {brace} and telemetry"),
    ] {
        assert_eq!(render(text).as_deref(), Some(want), "{text}");
    }
    assert!(render("{attachment}").unwrap().starts_with(r#"{"org""#));
    // No value for this message: the key is left out of the context.
    for text in [
        "{key.9}",
        "{key.-9}",
        "{attachment.absent}",
        "{attachment.nested}",
    ] {
        assert_eq!(render(text), None, "{text}");
    }
    let bare = Arrival::now("telemetry/sensor/radar-a/cot/track");
    let t = |text: &str| {
        Template::parse(text, 2)
            .unwrap()
            .render(&bare, sub, "telemetry/sensor/")
    };
    assert_eq!(t("{timestamp_at}"), None);
    assert_eq!(t("{attachment}"), None);
    assert_eq!(t("{attachment.org}"), None);
    assert_eq!(t("{capture.2}").as_deref(), Some("track"));
    assert_eq!(t("{encoding}").as_deref(), Some(""));
}

#[test]
fn captures_follow_the_subscribe_expression() {
    let c = |sub: &str, key: &str| captures(sub, key);
    assert_eq!(c("a/*/c", "a/b/c"), Some(vec!["b".to_string()]));
    assert_eq!(c("a/**", "a"), Some(vec![String::new()]));
    assert_eq!(c("a/**/z", "a/b/c/z"), Some(vec!["b/c".to_string()]));
    assert_eq!(
        c("a/**/x/*", "a/x/x/9"),
        Some(vec!["x".to_string(), "9".to_string()])
    );
    assert_eq!(c("a/t$*/c", "a/track/c"), Some(vec!["track".to_string()]));
    assert_eq!(c("a/*/c", "a/b/d"), None);
    assert_eq!(c("a/*", "a/b/c"), None);
}

#[test]
fn the_instant_text_is_utc_with_milliseconds() {
    assert_eq!(iso8601_ms(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(iso8601_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
    assert_eq!(iso8601_ms(1_709_251_199_999), "2024-02-29T23:59:59.999Z");
    assert_eq!(iso8601_ms(4_102_444_800_000), "2100-01-01T00:00:00.000Z");
    assert_eq!(iso8601_ms(-1), "1969-12-31T23:59:59.999Z");
}

#[test]
fn a_set_template_the_route_cannot_fill_is_refused_at_start() {
    let with = |set: &str| {
        format!(
            r#"[{{"name":"g","subscribe":"telemetry/sensor/*/cot/*","from":"cot-xml","to":"geo-track-v1","strip_prefix":"telemetry/sensor/","publish_prefix":"data/geo/track/v1/","set":{set}}}]"#
        )
    };
    refused(
        &with(r#"{"geotrack.kind":"{topic}"}"#),
        "is not a placeholder",
    );
    refused(&with(r#"{"geotrack.kind":"{capture.3}"}"#), "2 wildcard(s)");
    refused(&with(r#"{"geotrack.kind":"{capture.0}"}"#), "2 wildcard(s)");
    refused(
        &with(r#"{"geotrack.kind":"{key.last}"}"#),
        "not a chunk number",
    );
    refused(&with(r#"{"geotrack.kind":"{key.-1"}"#), "is not closed");
    refused(&with(r#"{"geotrack.kind":"track}"}"#), "with no `{`");
    refused(&with(r#"{"no.such_key":"{key.-1}"}"#), "undeclared-key");
    // A fixed value of the wrong type is refused at start, as before.
    refused(
        &with(r#"{"geotrack.synthetic":"yes"}"#),
        "key-type-mismatch",
    );
    assert!(table(&with(r#"{"geotrack.synthetic":"{attachment.synthetic}"}"#)).is_ok());
}

fn example_table() -> Table {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.json"))
            .unwrap();
    let cfg = Config::from_json(&text).unwrap();
    Table::build(&cfg, load_bundle(&bundle(), Distribution::Public).unwrap()).unwrap()
}

#[test]
fn the_example_route_produces_geo_track_v1_from_the_arrival() {
    let t = example_table();
    let idx = t
        .routes
        .iter()
        .position(|r| r.cfg.name == "cot-to-geo-track")
        .unwrap();
    let out = t
        .convert_arrival(
            idx,
            &arrival("telemetry/sensor/radar-a/cot/track"),
            COT.as_bytes(),
        )
        .unwrap();
    assert_eq!(out.key, "data/geo/track/v1/radar-a/cot/track");
    assert_eq!(out.encoding, "application/json;geo-track-v1");
    let v: serde_json::Value = serde_json::from_slice(&out.payload).unwrap();
    assert_eq!(v["schema"], "goat.geo.track.v1");
    assert_eq!(v["id"], "radar-a:4411");
    assert_eq!(v["label"], "4411");
    assert_eq!(v["kind"], "track");
    assert_eq!(v["source"]["sensor"], "radar-a");
    assert_eq!(v["source"]["site"], "zagros");
    assert_eq!(v["source"]["node"], "xf-1");
    assert_eq!(v["observed_at"], "2021-02-27T20:00:01.250Z");
    assert_eq!(v["reported_at"], "2021-02-27T20:00:00.250Z");
    assert_eq!(v["producer_anchor"], "00112233445566778899aabbccddeeff");
    assert_eq!(v["synthetic"], false);
    assert_eq!(v["org"], "Example Org");
    assert_eq!(v["lat"], 30.25);

    // The kind is the last chunk of the key the message arrived on.
    let alert = t
        .convert_arrival(
            idx,
            &arrival("telemetry/sensor/radar-b/cot/alert"),
            COT.as_bytes(),
        )
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&alert.payload).unwrap();
    assert_eq!(
        (v["kind"].as_str(), v["id"].as_str()),
        (Some("alert"), Some("radar-b:4411"))
    );

    // No attachment: `org` is not required, so the track is written without it.
    let mut bare = arrival("telemetry/sensor/radar-a/cot/track");
    bare.attachment = None;
    let v: serde_json::Value = serde_json::from_slice(
        &t.convert_arrival(idx, &bare, COT.as_bytes())
            .unwrap()
            .payload,
    )
    .unwrap();
    assert!(v.get("org").is_none());
}

#[test]
fn a_value_of_the_wrong_type_from_the_arrival_refuses_that_message() {
    let r = r#"[{"name":"g","subscribe":"telemetry/sensor/*/cot/*","from":"cot-xml","to":"geo-track-v1",
        "strip_prefix":"telemetry/sensor/","publish_prefix":"data/geo/track/v1/",
        "set":{"geotrack.kind":"{key.-1}","geotrack.sensor":"{capture.1}","geotrack.site":"s","geotrack.node":"n",
               "geotrack.observed_at":"{received_at}","geotrack.producer_anchor":"00112233445566778899aabbccddeeff",
               "geotrack.synthetic":"{attachment.org}"}}]"#;
    let t = table(r).unwrap();
    let err = t
        .convert_arrival(
            0,
            &arrival("telemetry/sensor/radar-a/cot/track"),
            COT.as_bytes(),
        )
        .unwrap_err();
    assert!(format!("{err:#}").contains("key-type-mismatch"), "{err:#}");
    assert_eq!(t.routes[0].counters.refused.load(Ordering::Relaxed), 1);
    // And with no attachment the required key is absent: the kind refuses it.
    let mut bare = arrival("telemetry/sensor/radar-a/cot/track");
    bare.attachment = None;
    let err = t.convert_arrival(0, &bare, COT.as_bytes()).unwrap_err();
    assert!(format!("{err:#}").contains("no-kind"), "{err:#}");
}

#[test]
fn a_table_that_would_feed_itself_is_refused() {
    let a = route(
        "a",
        "telemetry/cot/**",
        "cot-xml",
        "geojson",
        "telemetry/cot/",
        "telemetry/cot/out/",
    );
    refused(&format!("[{a}]"), "subscribes to");
    let a = route(
        "a",
        "telemetry/cot/zagros/*",
        "cot-xml",
        "cot-protobuf",
        "telemetry/cot/",
        "telemetry/takproto/",
    );
    let b = route(
        "b",
        "telemetry/takproto/**",
        "cot-protobuf",
        "cot-xml",
        "telemetry/takproto/",
        "telemetry/cot/",
    );
    refused(&format!("[{a},{b}]"), "subscribes to");
}

#[test]
fn start_up_checks_refuse_what_they_would_have_to_guess() {
    let ok = |sub: &str, from: &str, strip: &str, publish: &str| {
        route("a", sub, from, "geojson", strip, publish)
    };
    refused(
        &format!(
            "[{}]",
            ok(
                "telemetry/cot/*",
                "no-such-format",
                "telemetry/cot/",
                "data/x/"
            )
        ),
        "not in the pack bundle",
    );
    refused(
        &format!(
            "[{}]",
            ok("other/cot/*", "cot-xml", "telemetry/cot/", "data/x/")
        ),
        "does not begin with strip_prefix",
    );
    refused(
        &format!(
            "[{}]",
            ok("telemetry/cot/*", "cot-xml", "telemetry/*/", "data/x/")
        ),
        "literal key prefix",
    );
    refused(
        &format!(
            "[{}]",
            ok("telemetry/cot/*", "cot-xml", "telemetry/cot/", "data/x")
        ),
        "literal key prefix",
    );
    refused("[]", "no routes");
    let r = ok("telemetry/cot/*", "cot-xml", "telemetry/cot/", "data/x/");
    refused(&format!("[{r},{r}]"), "used twice");
    let bad_set = r#"[{"name":"a","subscribe":"telemetry/cot/*","from":"cot-xml","to":"geojson","strip_prefix":"telemetry/cot/","publish_prefix":"data/x/","set":{"no.such_key":"1"}}]"#;
    refused(bad_set, "undeclared-key");
    let unknown_member = r#"[{"name":"a","subscribe":"telemetry/cot/*","from":"cot-xml","to":"geojson","strip_prefix":"telemetry/cot/","publish_prefix":"data/x/","qos":"high"}]"#;
    refused(unknown_member, "not valid");
}

fn copy_bundle(to: &Path) {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let p = e.unwrap().path();
            let dst = to.join(p.file_name().unwrap());
            if p.is_dir() {
                copy(&p, &dst)
            } else {
                std::fs::copy(&p, &dst).map(|_| ()).unwrap()
            }
        }
    }
    copy(&bundle(), to);
}

#[test]
fn an_altered_or_extended_bundle_does_not_load() {
    let dir = tempfile::tempdir().unwrap();
    copy_bundle(dir.path());
    let pack = dir.path().join("packs/geojson/pack.def");
    let text = std::fs::read_to_string(&pack).unwrap();
    std::fs::write(&pack, format!("{text}# changed\n")).unwrap();
    let err = load_bundle(dir.path(), Distribution::Public).err().unwrap();
    assert!(format!("{err:#}").contains("does not match the manifest"));

    std::fs::write(&pack, text).unwrap();
    std::fs::create_dir_all(dir.path().join("packs/extra")).unwrap();
    std::fs::write(dir.path().join("packs/extra/pack.def"), "format extra\n").unwrap();
    let err = load_bundle(dir.path(), Distribution::Public).err().unwrap();
    assert!(format!("{err:#}").contains("not in the manifest"));
}

#[test]
fn a_controlled_pack_is_refused_at_the_public_limit() {
    let dir = tempfile::tempdir().unwrap();
    copy_bundle(dir.path());
    let pack = dir.path().join("packs/geojson/pack.def");
    let text = std::fs::read_to_string(&pack)
        .unwrap()
        .replace("distribution public", "distribution controlled");
    std::fs::write(&pack, &text).unwrap();
    // Record the changed file in the manifest, as a bundle built with a
    // controlled pack would.
    let manifest = dir.path().join("MANIFEST.json");
    let mut m: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
    for f in m["files"].as_array_mut().unwrap() {
        if f["path"] == "packs/geojson/pack.def" {
            f["sha256"] = hex::encode(Sha256::digest(text.as_bytes())).into();
        }
    }
    std::fs::write(&manifest, m.to_string()).unwrap();
    let err = load_bundle(dir.path(), Distribution::Public).err().unwrap();
    assert!(
        format!("{err:#}").contains("distribution-refused"),
        "{err:#}"
    );
    assert!(load_bundle(dir.path(), Distribution::Controlled).is_ok());
}

#[test]
fn the_three_statements_of_the_library_commit_agree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pin = std::fs::read_to_string(root.join("../../xf/goat-transformers.ref")).unwrap();
    let pin = pin.trim();
    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(
        cargo.contains(&format!(r#"rev = "{pin}""#)),
        "Cargo.toml does not pin {pin}"
    );
    assert_eq!(
        load_bundle(&bundle(), Distribution::Public)
            .unwrap()
            .library_rev,
        pin
    );
}
