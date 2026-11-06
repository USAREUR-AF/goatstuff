//! Synthetic certificates and loopback sockets only; no fleet credentials.
use rustls::{
    pki_types::{PrivateKeyDer, ServerName, UnixTime},
    RootCertStore,
};
use std::{path::Path, sync::Arc, time::Duration};

fn issuer() -> (rcgen::Issuer<'static, rcgen::KeyPair>, rcgen::Certificate) {
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    (rcgen::Issuer::new(params, key), cert)
}

#[test]
fn zenoh_verifier_rejects_distrusted_key_and_untrusted_issuer() {
    use rustls::client::danger::ServerCertVerifier;
    use zenoh_link_commons::tls::WebPkiVerifierAnyServerName;
    let (issuer, root) = issuer();
    let mut roots = RootCertStore::empty();
    roots.add(root.der().clone()).unwrap();
    // This is the verifier used by production's verify_name_on_connect=false.
    let verifier = WebPkiVerifierAnyServerName::new(roots);
    let name = ServerName::try_from("fixture.invalid").unwrap();
    let params = || rcgen::CertificateParams::new(vec!["fixture.invalid".into()]).unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let good = params().signed_by(&key, &issuer).unwrap();
    verifier
        .verify_server_cert(good.der(), &[], &name, &[], UnixTime::now())
        .unwrap();
    let untrusted = params().self_signed(&key).unwrap();
    assert!(verifier
        .verify_server_cert(untrusted.der(), &[], &name, &[], UnixTime::now())
        .is_err());
    let contents = include_bytes!("../tests/fixtures/EMDVTLSCAG3B.spki.der");
    assert!(contents.len() < 128);
    let mut der = vec![0x30, contents.len() as u8];
    der.extend_from_slice(contents);
    let public_key = rcgen::SubjectPublicKeyInfo::from_der(&der).unwrap();
    let blocked = params().signed_by(&public_key, &issuer).unwrap();
    let error = verifier
        .verify_server_cert(blocked.der(), &[], &name, &[], UnixTime::now())
        .unwrap_err();
    assert!(format!("{error:?}").contains("CertDistrusted"), "{error:?}");
}

#[test]
fn pinned_rustls_preserves_hybrid_and_classical_handshakes() {
    use std::net::{TcpListener, TcpStream};
    let (issuer, root) = issuer();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    for hybrid in [true, false] {
        let mut provider = rustls::crypto::aws_lc_rs::default_provider();
        // Require the negotiated group rather than merely offering it.
        provider.kx_groups.retain(|g| {
            if hybrid {
                g.name() == rustls::NamedGroup::X25519MLKEM768
            } else {
                g.name() == rustls::NamedGroup::secp256r1
            }
        });
        assert!(!provider.kx_groups.is_empty());
        let server = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
            )
            .unwrap();
        let mut roots = RootCertStore::empty();
        roots.add(root.der().clone()).unwrap();
        let client_provider = if hybrid {
            rustls::crypto::aws_lc_rs::default_provider()
        } else {
            rustls::crypto::ring::default_provider()
        };
        let client = rustls::ClientConfig::builder_with_provider(Arc::new(client_provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut connection = rustls::ServerConnection::new(Arc::new(server)).unwrap();
            while connection.is_handshaking() {
                connection.complete_io(&mut socket).unwrap();
            }
            connection.negotiated_key_exchange_group().unwrap().name()
        });
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut connection = rustls::ClientConnection::new(
            Arc::new(client),
            ServerName::try_from("localhost").unwrap(),
        )
        .unwrap();
        while connection.is_handshaking() {
            connection.complete_io(&mut socket).unwrap();
        }
        let expected = if hybrid {
            rustls::NamedGroup::X25519MLKEM768
        } else {
            rustls::NamedGroup::secp256r1
        };
        assert_eq!(
            connection.negotiated_key_exchange_group().unwrap().name(),
            expected
        );
        assert_eq!(thread.join().unwrap(), expected);
    }
}

fn configure(config: &mut zenoh::Config, path: &str, value: serde_json::Value) {
    config.insert_json5(path, &value.to_string()).unwrap();
}

// Separate process: the router uses the PQ-capable provider, while the real
// peer retains its normal Zenoh-selected ring provider. Never change the
// parent process provider to make this test connect.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn router_fixture() {
    let Some(root) = std::env::var_os("SURFACE_TLS_ROUTER_FIXTURE") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .unwrap();
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
        "transport/link/tls",
        serde_json::json!({
            "root_ca_certificate": root.join("ca.pem"), "listen_certificate": root.join("server.pem"),
            "listen_private_key": root.join("server-key.pem"), "enable_mtls":true
        }),
    );
    let session = tokio::time::timeout(Duration::from_secs(10), zenoh::open(config))
        .await
        .unwrap()
        .unwrap();
    let subscriber = session
        .declare_subscriber("test/surface/tls")
        .await
        .unwrap();
    std::fs::write(root.join("ready"), "ready").unwrap();
    let sample = tokio::time::timeout(Duration::from_secs(30), subscriber.recv_async())
        .await
        .unwrap()
        .unwrap();
    std::fs::write(root.join("received"), sample.payload().to_bytes()).unwrap();
    session.close().await.unwrap();
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn wait_for(path: &Path) {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "fixture timeout: {}",
        std::fs::read_to_string(path.parent().unwrap().join("router.log")).unwrap_or_default()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_peer_connects_and_publishes_over_mtls() {
    use clap::Parser;
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let (issuer, ca) = issuer();
    std::fs::write(root.join("ca.pem"), ca.pem()).unwrap();
    for name in ["server", "client"] {
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
            .unwrap()
            .signed_by(&key, &issuer)
            .unwrap();
        std::fs::write(root.join(format!("{name}.pem")), cert.pem()).unwrap();
        std::fs::write(root.join(format!("{name}-key.pem")), key.serialize_pem()).unwrap();
    }
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let endpoint = format!("tls/127.0.0.1:{}", listener.local_addr().unwrap().port());
    drop(listener);
    std::fs::write(root.join("endpoint"), &endpoint).unwrap();
    let log = std::fs::File::create(root.join("router.log")).unwrap();
    let _child = Child(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tls_tests::router_fixture", "--nocapture"])
            .env("SURFACE_TLS_ROUTER_FIXTURE", root)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    wait_for(&root.join("ready")).await;
    let args = crate::Args::try_parse_from([
        "fixture",
        "--surface",
        "fixture",
        "--env",
        "fixture",
        "--node",
        "fixture",
        "--age-recipients-file",
        "unused",
        "--endpoint",
        &endpoint,
        "--cert",
        root.join("client.pem").to_str().unwrap(),
        "--key",
        root.join("client-key.pem").to_str().unwrap(),
        "--root-ca",
        root.join("ca.pem").to_str().unwrap(),
    ])
    .unwrap();
    let session = tokio::time::timeout(Duration::from_secs(15), crate::open_session(&args))
        .await
        .unwrap()
        .unwrap();
    session
        .put("test/surface/tls", "synthetic fixture")
        .await
        .unwrap();
    wait_for(&root.join("received")).await;
    assert_eq!(
        std::fs::read(root.join("received")).unwrap(),
        b"synthetic fixture"
    );
    session.close().await.unwrap();
}
