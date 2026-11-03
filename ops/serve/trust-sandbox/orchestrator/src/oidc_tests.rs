//! Exercise the production JWKS/RS256 verification path with synthetic keys.
use crate::oidc::Oidc;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

fn rsa_fixture() -> (EncodingKey, String) {
    // OpenSSL generates synthetic test material only; no private key is
    // checked in or written to disk.
    let key = Command::new("openssl")
        .args([
            "genpkey",
            "-algorithm",
            "RSA",
            "-pkeyopt",
            "rsa_keygen_bits:2048",
            "-pkeyopt",
            "rsa_keygen_pubexp:65537",
        ])
        .output()
        .unwrap();
    assert!(key.status.success());
    let mut modulus = Command::new("openssl")
        .args(["rsa", "-noout", "-modulus"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    modulus
        .stdin
        .take()
        .unwrap()
        .write_all(&key.stdout)
        .unwrap();
    let modulus = modulus.wait_with_output().unwrap();
    assert!(modulus.status.success());
    let text = std::str::from_utf8(&modulus.stdout).unwrap().trim();
    let text = text.strip_prefix("Modulus=").unwrap();
    assert!(text.len().is_multiple_of(2));
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect();
    (
        EncodingKey::from_rsa_pem(&key.stdout).unwrap(),
        URL_SAFE_NO_PAD.encode(bytes),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_oidc_accepts_rs256_and_rejects_invalid_tokens() {
    let (key, modulus) = rsa_fixture();
    let jwks = json!({"keys":[{
        "kty":"RSA", "kid":"fixture-key", "use":"sig", "alg":"RS256",
        "n":modulus, "e":"AQAB"
    }]});
    let app = axum::Router::new().route(
        "/.well-known/jwks.json",
        axum::routing::get(move || {
            let jwks = jwks.clone();
            async move { axum::Json(jwks) }
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let local = tokio::task::LocalSet::new();
    let server = local.spawn_local(async move {
        axum::serve(listener, app).await.unwrap();
    });
    local.run_until(async {
    let issuer = "https://issuer.fixture.invalid";
    let client = Oidc::new(
        "http://unused.fixture.invalid",
        &endpoint,
        issuer,
        "fixture-client",
    );
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;
    let good = json!({"sub":"fixture-actor", "groups":["ADMIN"], "iss":issuer, "aud":"fixture-client", "exp":exp});
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("fixture-key".into());
    let token = |claims: &Value| encode(&header, claims, &key).unwrap();
    let valid = token(&good);
    let claims = client.verify(&valid).await.unwrap();
    assert_eq!(claims.sub, "fixture-actor");
    assert_eq!(claims.groups, vec!["ADMIN"]);
    assert_eq!(claims.exp as u64, exp);
    for (field, value) in [
        ("iss", json!("https://other.fixture.invalid")),
        ("aud", json!("other-client")),
        ("exp", json!(0)),
    ] {
        let mut invalid = good.clone();
        invalid[field] = value;
        assert!(
            client.verify(&token(&invalid)).await.is_err(),
            "accepted invalid {field}"
        );
    }
    let mut missing_exp = good.clone();
    missing_exp.as_object_mut().unwrap().remove("exp");
    assert!(client.verify(&token(&missing_exp)).await.is_err());
    let mut other_kid = header.clone();
    other_kid.kid = Some("other-key".into());
    assert!(client
        .verify(&encode(&other_kid, &good, &key).unwrap())
        .await
        .is_err());
    let mut no_kid = header.clone();
    no_kid.kid = None;
    assert!(client
        .verify(&encode(&no_kid, &good, &key).unwrap())
        .await
        .is_err());
    let mut hmac = Header::new(Algorithm::HS256);
    hmac.kid = header.kid.clone();
    let token = encode(
        &hmac,
        &good,
        &EncodingKey::from_secret(b"synthetic fixture secret"),
    )
    .unwrap();
    assert!(
        client.verify(&token).await.is_err(),
        "accepted algorithm confusion"
    );
    let mut parts: Vec<String> = valid.split('.').map(str::to_owned).collect();
    let first = if parts[2].starts_with('A') { "B" } else { "A" };
    parts[2].replace_range(..1, first);
    assert!(client.verify(&parts.join(".")).await.is_err());
    let (other_key, _) = rsa_fixture();
    assert!(client
        .verify(&encode(&header, &good, &other_key).unwrap())
        .await
        .is_err());
    assert!(client.verify("malformed-fixture-token").await.is_err());
    }).await;
    server.abort();
}
