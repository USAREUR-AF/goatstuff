//! Exercise the application's S3 factory and default HTTPS connector in child
//! processes. Each child gets its own trust-store environment before startup.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::time::timeout;

const PAYLOAD: &str = "{\"fixture\":\"audit-tls-contract\"}\n";

#[test]
fn request_child() {
    let Ok(endpoint) = std::env::var("GOAT_S3_TLS_TEST_ENDPOINT") else {
        return;
    };
    let expected = std::env::var("GOAT_S3_TLS_TEST_EXPECT").unwrap();
    let args = WriterArgs {
        s3_endpoint: Some(endpoint),
        s3_bucket: "audit-fixture".into(),
        s3_region: "us-east-1".into(),
        s3_access_key: Some("fixture-access".into()),
        s3_secret_key: Some("fixture-secret".into()),
        audit_subscriber_id: "fixture-subscriber".into(),
        batch_events: 1,
        batch_interval: Duration::from_secs(1),
        local_ndjson_dir: None,
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let client = build_s3_client(&args).await.unwrap().unwrap();
            // One connection per fixture: retain the certificate error instead
            // of retrying against a listener that has already refused TLS.
            let client = aws_sdk_s3::Client::from_conf(
                client
                    .config()
                    .to_builder()
                    .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
                    .build(),
            );
            let result = client
                .put_object()
                .bucket(&args.s3_bucket)
                .key("fixture.ndjson")
                .content_type("application/x-ndjson")
                .body(ByteStream::from_static(PAYLOAD.as_bytes()))
                .send()
                .await;
            match expected.as_str() {
                "ok" => assert!(result.is_ok(), "{result:?}"),
                "issuer" => {
                    let error = format!("{:?}", result.unwrap_err());
                    assert!(error.contains("UnknownIssuer"), "{error}");
                }
                "name" => {
                    let error = format!("{:?}", result.unwrap_err());
                    assert!(error.contains("NotValidForName"), "{error}");
                }
                _ => panic!("unknown test expectation"),
            }
        });
}

#[tokio::test]
async fn default_s3_https_client_preserves_upload_and_certificate_checks() {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let other_key = rcgen::KeyPair::generate().unwrap();
    let mut other_params = rcgen::CertificateParams::new(vec!["127.0.0.1".into()]).unwrap();
    other_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "untrusted fixture root");
    let other_cert = other_params.self_signed(&other_key).unwrap();
    let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
    let fixture = tempfile::tempdir().unwrap();
    let trust_file = fixture.path().join("roots.pem");
    let empty_dir = fixture.path().join("empty-roots");
    std::fs::create_dir(&empty_dir).unwrap();

    for (expected, host, roots) in [
        ("ok", "127.0.0.1", cert.pem()),
        ("issuer", "127.0.0.1", other_cert.pem()),
        ("name", "localhost", cert.pem()),
    ] {
        std::fs::write(&trust_file, roots).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let acceptor = acceptor.clone();
        let server = async move {
            let (socket, _) = timeout(Duration::from_secs(15), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let connection = timeout(Duration::from_secs(10), acceptor.accept(socket))
                .await
                .unwrap();
            if expected != "ok" {
                assert!(connection.is_err(), "invalid certificate was accepted");
                return Vec::new();
            }
            let mut connection = connection.unwrap();
            let mut request = Vec::new();
            timeout(Duration::from_secs(10), async {
                loop {
                    let mut bytes = [0; 4096];
                    let count = connection.read(&mut bytes).await.unwrap();
                    assert!(count > 0, "request ended before its body");
                    request.extend_from_slice(&bytes[..count]);
                    if let Some(end) = request.windows(4).position(|p| p == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .expect("ByteStream fixture must declare its encoded body length");
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                    assert!(request.len() < 65536, "unbounded fixture request");
                }
                connection
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
            })
            .await
            .unwrap();
            request
        };
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .kill_on_drop(true)
            .args(["--exact", "s3_tls_tests::request_child", "--nocapture"])
            .env(
                "GOAT_S3_TLS_TEST_ENDPOINT",
                format!("https://{host}:{port}"),
            )
            .env("GOAT_S3_TLS_TEST_EXPECT", expected)
            .env("SSL_CERT_FILE", &trust_file)
            .env("SSL_CERT_DIR", &empty_dir)
            .env("AWS_EC2_METADATA_DISABLED", "true")
            .env("AWS_CONFIG_FILE", fixture.path().join("no-config"))
            .env(
                "AWS_SHARED_CREDENTIALS_FILE",
                fixture.path().join("no-credentials"),
            )
            .env("NO_PROXY", "*")
            .env("no_proxy", "*")
            .env_remove("HTTPS_PROXY")
            .env_remove("HTTP_PROXY")
            .env_remove("ALL_PROXY")
            .env_remove("https_proxy")
            .env_remove("http_proxy")
            .env_remove("all_proxy");
        let (output, request) = tokio::join!(
            timeout(Duration::from_secs(30), command.output()),
            timeout(Duration::from_secs(30), server),
        );
        let output = output.unwrap().unwrap();
        assert!(
            output.status.success(),
            "{expected}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let request = request.unwrap();
        if expected == "ok" {
            let request = String::from_utf8(request).unwrap();
            let line = request.lines().next().unwrap();
            let parts: Vec<_> = line.split_whitespace().collect();
            assert_eq!(parts[0], "PUT");
            assert_eq!(
                parts[1].split('?').next().unwrap(),
                "/audit-fixture/fixture.ndjson"
            );
            assert_eq!(parts[2], "HTTP/1.1");
            assert!(request
                .to_lowercase()
                .contains("authorization: aws4-hmac-sha256"));
            assert!(request
                .to_lowercase()
                .contains("content-type: application/x-ndjson"));
            assert!(request.contains(PAYLOAD));
        }
    }
}

#[test]
fn active_s3_verifier_rejects_a_known_distrusted_emsign_public_key() {
    use rustls::client::danger::ServerCertVerifier;
    use rustls::pki_types::{ServerName, UnixTime};

    let root_key = rcgen::KeyPair::generate().unwrap();
    let mut root_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    root_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    root_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "synthetic trusted fixture root");
    let root = root_params.self_signed(&root_key).unwrap();
    let issuer = rcgen::Issuer::new(root_params, root_key);
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.der().clone()).unwrap();
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
    )
    .build()
    .unwrap();
    let name = ServerName::try_from("127.0.0.1").unwrap();
    let good_key = rcgen::KeyPair::generate().unwrap();
    let good = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
        .unwrap()
        .signed_by(&good_key, &issuer)
        .unwrap();
    verifier
        .verify_server_cert(good.der(), &[], &name, &[], UnixTime::now())
        .unwrap();

    // Upstream's fixture contains the SPKI SEQUENCE contents. Restore its
    // short-form outer tag/length for rcgen's public-key parser. No private
    // emSign key is needed: our synthetic trusted issuer signs this certificate.
    let contents = include_bytes!("../tests/fixtures/EMDVTLSCAG3B.spki.der");
    assert!(contents.len() < 128);
    let mut spki = vec![0x30, contents.len() as u8];
    spki.extend_from_slice(contents);
    let distrusted_key = rcgen::SubjectPublicKeyInfo::from_der(&spki).unwrap();
    let distrusted = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
        .unwrap()
        .signed_by(&distrusted_key, &issuer)
        .unwrap();
    let error = verifier
        .verify_server_cert(distrusted.der(), &[], &name, &[], UnixTime::now())
        .unwrap_err();
    assert!(format!("{error:?}").contains("CertDistrusted"), "{error:?}");
}
