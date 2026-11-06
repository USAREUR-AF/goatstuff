//! Real feeds executable against a separate, unmodified Zenoh 1.0.4 router.
//! Run through transport_compatibility.py so the router dependency is independent.
use std::{
    path::Path,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

fn configure(config: &mut zenoh::Config, path: &str, value: serde_json::Value) {
    config.insert_json5(path, &value.to_string()).unwrap();
}

#[ignore = "fixture invoked explicitly by the compatibility test"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upstream_router() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init()
        .unwrap();
    let root =
        std::path::PathBuf::from(std::env::var_os("FEEDS_ROUTER_FIXTURE").expect("fixture root"));
    let mut config = zenoh::Config::default();
    configure(&mut config, "mode", serde_json::json!("router"));
    configure(
        &mut config,
        "listen/endpoints",
        serde_json::json!([std::fs::read_to_string(root.join("endpoint")).unwrap()]),
    );
    configure(
        &mut config,
        "scouting/multicast/enabled",
        serde_json::json!(false),
    );
    configure(
        &mut config,
        "scouting/gossip/enabled",
        serde_json::json!(false),
    );
    configure(
        &mut config,
        "transport/unicast/compression/enabled",
        serde_json::from_str(&std::fs::read_to_string(root.join("compression")).unwrap()).unwrap(),
    );
    configure(
        &mut config,
        "transport/link/tls",
        serde_json::json!({
            "root_ca_certificate":root.join("ca.pem"), "listen_certificate":root.join("server.pem"),
            "listen_private_key":root.join("server-key.pem"), "enable_mtls":true
        }),
    );
    let session = tokio::time::timeout(Duration::from_secs(10), zenoh::open(config))
        .await
        .unwrap()
        .unwrap();
    let subscriber = session
        .declare_subscriber("attest/queryable/feeds/fixture/feed")
        .await
        .unwrap();
    std::fs::write(root.join("ready"), "ready").unwrap();
    let sample = tokio::time::timeout(Duration::from_secs(30), subscriber.recv_async())
        .await
        .unwrap()
        .unwrap();
    std::fs::write(root.join("received.pending"), sample.payload().to_bytes()).unwrap();
    std::fs::rename(root.join("received.pending"), root.join("received")).unwrap();
    session.close().await.unwrap();
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "fixture timeout: {}",
            std::fs::read_to_string(path.parent().unwrap().join("router.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn ca(name: &str) -> (rcgen::Issuer<'static, rcgen::KeyPair>, rcgen::Certificate) {
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, name);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    (rcgen::Issuer::new(params, key), cert)
}

struct HttpFixture {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<usize>>,
    url: String,
}
impl HttpFixture {
    fn start() -> Self {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut requests = 0;
            while !done.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        let mut input = Vec::new();
                        while !input.ends_with(b"\r\n\r\n") {
                            let mut chunk = [0; 512];
                            let count = stream.read(&mut chunk).unwrap();
                            assert!(count > 0 && input.len() + count <= 4096);
                            input.extend_from_slice(&chunk[..count]);
                        }
                        assert!(input.starts_with(b"GET /v3/paths/list HTTP/1.1\r\n"));
                        let body = serde_json::json!({"items":[{"name":"feed","ready":true,"inboundBytes":42,"outboundBytes":64,"source":{"type":"rtsp"},"tracks2":[{"codec":"H264","codecProps":{"large":"synthetic".repeat(4096)}}]}]}).to_string();
                        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
                        requests += 1;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => panic!("fixture accept: {e}"),
                }
            }
            requests
        });
        Self {
            stop,
            thread: Some(thread),
            url,
        }
    }
    fn finish(&mut self) -> usize {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap()
    }
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.finish();
        }
    }
}

#[ignore = "run tests/transport_compatibility.py to build the independent upstream router"]
#[test]
fn production_binary_preserves_tls_quic_and_compression() {
    let router = std::env::var_os("FEEDS_UPSTREAM_TEST_BIN")
        .expect("independent router fixture required; run transport_compatibility.py");
    let producer = std::env::var_os("FEEDS_PRODUCER_BIN")
        .expect("production feeds executable required; run transport_compatibility.py");
    for transport in ["tls", "quic"] {
        for case in [
            "named",
            "any-name-compressed",
            "wrong-name",
            "untrusted",
            "missing-client",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let (issuer, cert) = ca("trusted-fixture");
            std::fs::write(root.join("ca.pem"), cert.pem()).unwrap();
            let (_, wrong_ca) = ca("untrusted-fixture");
            std::fs::write(root.join("wrong-ca.pem"), wrong_ca.pem()).unwrap();
            let any_name = case == "any-name-compressed" || case == "untrusted";
            let wrong_name = any_name || case == "wrong-name";
            for role in ["server", "client"] {
                let key = rcgen::KeyPair::generate().unwrap();
                let name = if role == "server" && wrong_name {
                    "fixture.invalid"
                } else {
                    "127.0.0.1"
                };
                let cert = rcgen::CertificateParams::new(vec![name.into()])
                    .unwrap()
                    .signed_by(&key, &issuer)
                    .unwrap();
                std::fs::write(root.join(format!("{role}.pem")), cert.pem()).unwrap();
                std::fs::write(root.join(format!("{role}-key.pem")), key.serialize_pem()).unwrap();
            }
            let port = if transport == "tls" {
                std::net::TcpListener::bind(("127.0.0.1", 0))
                    .unwrap()
                    .local_addr()
                    .unwrap()
                    .port()
            } else {
                std::net::UdpSocket::bind(("127.0.0.1", 0))
                    .unwrap()
                    .local_addr()
                    .unwrap()
                    .port()
            };
            let endpoint = format!("{transport}/127.0.0.1:{port}");
            std::fs::write(root.join("endpoint"), &endpoint).unwrap();
            std::fs::write(root.join("compression"), any_name.to_string()).unwrap();
            let log = std::fs::File::create(root.join("router.log")).unwrap();
            let _server = Child(
                Command::new(&router)
                    .args(["--ignored", "--exact", "upstream_router", "--nocapture"])
                    .env("FEEDS_ROUTER_FIXTURE", root)
                    .stdout(log.try_clone().unwrap())
                    .stderr(log)
                    .spawn()
                    .unwrap(),
            );
            wait_for(&root.join("ready"));
            let mut config = zenoh::Config::default();
            configure(&mut config, "mode", serde_json::json!("client"));
            configure(
                &mut config,
                "connect/endpoints",
                serde_json::json!([endpoint]),
            );
            configure(&mut config, "connect/timeout_ms", serde_json::json!(2000));
            configure(
                &mut config,
                "connect/exit_on_failure",
                serde_json::json!(true),
            );
            configure(
                &mut config,
                "scouting/multicast/enabled",
                serde_json::json!(false),
            );
            configure(
                &mut config,
                "scouting/gossip/enabled",
                serde_json::json!(false),
            );
            configure(
                &mut config,
                "transport/unicast/compression/enabled",
                serde_json::json!(any_name),
            );
            let mut tls = serde_json::json!({"root_ca_certificate":root.join(if case=="untrusted" {"wrong-ca.pem"} else {"ca.pem"}),"verify_name_on_connect":!any_name,"enable_mtls":case!="missing-client"});
            if case != "missing-client" {
                tls["connect_certificate"] = serde_json::json!(root.join("client.pem"));
                tls["connect_private_key"] = serde_json::json!(root.join("client-key.pem"));
            }
            configure(&mut config, "transport/link/tls", tls);
            std::fs::write(
                root.join("client.json"),
                serde_json::to_vec(&config).unwrap(),
            )
            .unwrap();
            let mut http = HttpFixture::start();
            let log = std::fs::File::create(root.join("client.log")).unwrap();
            let mut client = Child(
                Command::new(&producer)
                    .env(
                        "RUST_LOG",
                        "zenoh=debug,zenoh_link_tls=debug,zenoh_link_quic=debug,quinn_proto=debug",
                    )
                    .args([
                        "--site",
                        "fixture",
                        "--poll-sec",
                        "1",
                        "--mediamtx-url",
                        &http.url,
                        "--zenoh-config",
                        root.join("client.json").to_str().unwrap(),
                    ])
                    .stdout(log.try_clone().unwrap())
                    .stderr(log)
                    .spawn()
                    .unwrap(),
            );
            let success = case == "named" || case == "any-name-compressed";
            let deadline = Instant::now() + Duration::from_secs(15);
            if success {
                while !root.join("received").exists() {
                    assert!(
                        client.0.try_wait().unwrap().is_none(),
                        "{transport}/{case}: {}",
                        std::fs::read_to_string(root.join("client.log")).unwrap()
                    );
                    assert!(
                        Instant::now() < deadline,
                        "{transport}/{case}: {}",
                        std::fs::read_to_string(root.join("client.log")).unwrap()
                    );
                    std::thread::sleep(Duration::from_millis(25));
                }
                let body: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(root.join("received")).unwrap()).unwrap();
                assert_eq!(body["name"], "feed");
                assert_eq!(body["site"], "fixture");
                assert_eq!(body["ready"], true);
                assert_eq!(body["inbound_bytes"], 42);
                assert_eq!(body["outbound_bytes"], 64);
                assert_eq!(body["schema_version"], 1);
                assert_eq!(body["source_type"], "rtsp");
                assert_eq!(body["tracks"][0]["codec"], "H264");
                assert_eq!(
                    body["tracks"][0]["codec_props"]["large"],
                    "synthetic".repeat(4096)
                );
                assert!(http.finish() > 0);
            } else {
                loop {
                    if let Some(status) = client.0.try_wait().unwrap() {
                        assert!(
                            !status.success(),
                            "{transport}/{case}: accepted invalid TLS fixture"
                        );
                        break;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "{transport}/{case}: did not fail closed: {}",
                        std::fs::read_to_string(root.join("client.log")).unwrap()
                    );
                    std::thread::sleep(Duration::from_millis(25));
                }
                assert!(!root.join("received").exists());
                assert_eq!(http.finish(), 0);
                let diagnostics = std::fs::read_to_string(root.join("client.log")).unwrap()
                    + &std::fs::read_to_string(root.join("router.log")).unwrap();
                let reason = match case {
                    "wrong-name" => "certificate not valid for name",
                    "untrusted" => "UnknownIssuer",
                    "missing-client" => "peer sent no certificates",
                    _ => unreachable!(),
                };
                assert!(diagnostics.contains(reason), "{transport}/{case}: missing expected certificate failure {reason}: {diagnostics}");
                println!("REJECT {transport}/{case}: {reason}");
            }
            println!("PASS {transport}/{case}");
        }
    }
}
