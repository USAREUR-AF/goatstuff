//! Synthetic certificates and loopback sockets only; no fleet credentials.
use rustls::{
    pki_types::{ServerName, UnixTime},
    RootCertStore,
};

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
    let contents = include_bytes!("fixtures/EMDVTLSCAG3B.spki.der");
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
