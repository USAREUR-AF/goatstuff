`EMDVTLSCAG3B.spki.der` is public SubjectPublicKeyInfo sequence content from
rustls/webpki commit `8ecdcc76f56b68fda67a6adf7976f5eed5006a4c`, distributed in
rustls-webpki 0.103.16. It contains a distrusted emSign public key, no private key.

Source: https://github.com/rustls/webpki/blob/8ecdcc76f56b68fda67a6adf7976f5eed5006a4c/src/data/distrusted/EMDVTLSCAG3B.spki.der

SHA-256: `2d5f988ee3ceaf696845a34299ca3e05816429cdb5de92427889533175f97b95`.

The regression test issues a synthetic certificate containing this public key
under its own temporary trusted CA. The old verifier accepts that valid chain;
the patched verifier must return `CertDistrusted`. All signing keys used by the
test are newly generated synthetic keys.
