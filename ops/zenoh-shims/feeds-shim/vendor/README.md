# Zenoh 1.0.4 certificate adapters

These three published Zenoh 1.0.4 crates keep the feeds client on its existing
protocol/API family and retain its transport/compression features. Their Rust
source is byte-identical to the registry archives. Only each normalized
Cargo.toml changes: rustls-webpki 0.102.8 becomes exact 0.103.16, the patched
version already used by Rustls in this workspace. No cryptographic code is forked.

`provenance.json` records registry archive checksums from the original lock,
upstream commit, every original file hash and each edited manifest hash.
LICENSE and NOTICE.md are the upstream files at that exact source revision.
The original Cargo.toml.orig is retained for provenance and is not built.

The compatibility test runs the production feeds executable against separately
built registry-only Zenoh 1.0.4 adapters. It checks TLS and QUIC, named and
configured any-name verification, compressed payload delivery, unknown issuers
and required client certificates. An independent verifier regression checks
the published distrusted key. These synthetic tests do not establish live fleet
acceptance or deployment.

Keep these copies until a reviewed upstream dependency resolution can replace
them without restoring the old verifier helper or regressing compression.
Run tests/check_vendor_provenance.py after any dependency maintenance.
