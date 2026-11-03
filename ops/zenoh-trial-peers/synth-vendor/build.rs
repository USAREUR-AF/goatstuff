// SPDX-License-Identifier: Apache-2.0
//
// Compile the notional personality .proto sources into Rust types so the
// synth-vendor binary can encode its tracks_demo / power_demo / beacon_demo
// payloads as protobuf bytes that match the topic.format declarations the
// portal serves and the .fds bodies panoscope's decoder fetches.
//
// Proto sources are vendored under ./proto-notional/. The canonical copies
// live at ops/zenoh-trial-peers/synth-vendor-schemas/proto-notional/ — the
// build script in that directory rebuilds the FDS bodies the portal serves.
// Both directories are kept in sync by hand for now; a CI check that diffs
// the two trees lands in a follow-on if drift bites.

fn main() {
    let proto_dir = "proto-notional";
    let protos = [
        "proto-notional/notional_tracks_demo_v1.proto",
        "proto-notional/notional_power_demo_v1.proto",
        "proto-notional/notional_beacon_demo_v1.proto",
    ];
    for p in &protos {
        println!("cargo:rerun-if-changed={p}");
    }
    println!("cargo:rerun-if-changed={proto_dir}");

    // Tell prost-build where to find protoc. We bundle one via
    // `protoc-bin-vendored` so the build is hermetic — Chainguard's
    // rust image doesn't ship protoc, and we'd rather avoid an
    // apk-add per-arch in the Dockerfile.
    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path().unwrap());

    prost_build::Config::new()
        .compile_protos(&protos, &[proto_dir])
        .expect("compile notional .proto sources");
}
