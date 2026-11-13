// SPDX-License-Identifier: Apache-2.0
//
// Environment-health + footprint wire types (decode side), for the
// web-native `top` view (docs/design/goat-top.md §"Web-native").
//
// Hand-written prost-derive mirrors of the on-bus health signal
// (ADR 1034/1036/1042). Tags MUST stay in lock-step with:
//   ops/zenoh-trial-peers/fabric-health/proto/fabric_health_v1.proto
//   ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto
// goat-cli carries the same mirrors in crates/goat-cli/src/health_wire.rs
// (crate-private there); both are decode-only consumers of the proto SoT.

/// Key prefixes the /api/top assembly scans in the sample buffer.
pub const HEALTH_PREFIX: &str = "ops/fabric/health/";
pub const FOOTPRINT_PREFIX: &str = "ops/fabric/footprint/";
pub const MESH_HEALTH_PREFIX: &str = "ops/mesh/health/";
pub const TIME_HEALTH_PREFIX: &str = "ops/time/health/";
pub const PRESENCE_HEALTH_PREFIX: &str = "ops/presence/health/";
pub const PEER_HEALTH_PREFIX: &str = "ops/peer/health/";

// --- fabric domain (ADR 1034) ---

#[derive(Clone, PartialEq, prost::Message)]
pub struct ClassHealthV1 {
    #[prost(string, tag = "1")]
    pub class: String,
    #[prost(double, tag = "2")]
    pub sample_rate: f64,
    #[prost(double, tag = "3")]
    pub byte_rate: f64,
    #[prost(double, tag = "4")]
    pub target_sample_rate: f64,
    #[prost(double, tag = "5")]
    pub target_byte_rate: f64,
    #[prost(double, tag = "6")]
    pub headroom_pct: f64,
    #[prost(string, tag = "7")]
    pub color: String,
    #[prost(uint64, tag = "8")]
    pub sample_count: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct FabricHealthV1 {
    #[prost(string, tag = "1")]
    pub site: String,
    #[prost(int64, tag = "2")]
    pub ts_unix_ms: i64,
    #[prost(message, repeated, tag = "3")]
    pub classes: Vec<ClassHealthV1>,
    #[prost(string, tag = "4")]
    pub overall_color: String,
    #[prost(uint32, tag = "5")]
    pub active_producers: u32,
    #[prost(uint32, tag = "6")]
    pub window_secs: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ClassShareV1 {
    #[prost(string, tag = "1")]
    pub class: String,
    #[prost(double, tag = "2")]
    pub sample_rate: f64,
    #[prost(double, tag = "3")]
    pub byte_rate: f64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ProducerFootprintV1 {
    #[prost(string, tag = "1")]
    pub peer: String,
    #[prost(int64, tag = "2")]
    pub ts_unix_ms: i64,
    #[prost(double, tag = "3")]
    pub sample_rate: f64,
    #[prost(double, tag = "4")]
    pub byte_rate: f64,
    #[prost(message, repeated, tag = "5")]
    pub shares: Vec<ClassShareV1>,
    #[prost(uint32, tag = "6")]
    pub rank: u32,
    #[prost(bool, tag = "7")]
    pub near_limit: bool,
}

// --- EnvHealthV1 envelope + per-domain detail (ADR 1036) ---

#[derive(Clone, PartialEq, prost::Message)]
pub struct MeshLinkHealthV1 {
    #[prost(string, tag = "1")]
    pub peer_fqdn: String,
    #[prost(string, tag = "2")]
    pub peer_ip: String,
    #[prost(string, tag = "3")]
    pub color: String,
    #[prost(bool, tag = "4")]
    pub connected: bool,
    #[prost(bool, tag = "5")]
    pub direct: bool,
    #[prost(string, tag = "6")]
    pub conn_type: String,
    #[prost(double, tag = "7")]
    pub latency_ms: f64,
    #[prost(string, tag = "8")]
    pub relay_address: String,
    #[prost(int64, tag = "9")]
    pub handshake_age_ms: i64,
    #[prost(uint64, tag = "10")]
    pub transition_count: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct MeshDetailV1 {
    #[prost(message, repeated, tag = "1")]
    pub links: Vec<MeshLinkHealthV1>,
    #[prost(string, tag = "2")]
    pub vantage_host: String,
    #[prost(uint32, tag = "3")]
    pub peers_total: u32,
    #[prost(uint32, tag = "4")]
    pub peers_connected: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct FabricDetailStub {}

#[derive(Clone, PartialEq, prost::Message)]
pub struct HostClockHealthV1 {
    #[prost(string, tag = "1")]
    pub host_id: String,
    #[prost(string, tag = "2")]
    pub color: String,
    #[prost(string, tag = "3")]
    pub verdict: String,
    #[prost(string, repeated, tag = "4")]
    pub verdict_reasons: Vec<String>,
    #[prost(double, tag = "5")]
    pub rms_offset_ms: f64,
    #[prost(int32, tag = "6")]
    pub stratum: i32,
    #[prost(bool, tag = "7")]
    pub gnss_present: bool,
    #[prost(double, tag = "8")]
    pub peer_median_offset_ms: f64,
    #[prost(bool, tag = "9")]
    pub divergence_detected: bool,
    #[prost(int64, tag = "10")]
    pub attestation_age_ms: i64,
    #[prost(bool, tag = "11")]
    pub attested: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TimeDetailV1 {
    #[prost(message, repeated, tag = "1")]
    pub hosts: Vec<HostClockHealthV1>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PresencePeerV1 {
    #[prost(string, tag = "1")]
    pub token_key: String,
    #[prost(string, tag = "2")]
    pub write_root: String,
    #[prost(string, tag = "3")]
    pub label: String,
    #[prost(bool, tag = "4")]
    pub alive: bool,
    #[prost(int64, tag = "5")]
    pub since_unix_ms: i64,
    #[prost(uint64, tag = "6")]
    pub drop_count: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PresenceDetailV1 {
    #[prost(message, repeated, tag = "1")]
    pub peers: Vec<PresencePeerV1>,
    #[prost(uint32, tag = "2")]
    pub alive_total: u32,
    #[prost(uint32, tag = "3")]
    pub dropped_recent: u32,
}

/// Peer domain (goatcore ADR 1232): one fault row per Zenoh ID that our
/// router saw stamp a rejected timestamp. Published by goat-peer-health.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PeerFaultV1 {
    #[prost(string, tag = "1")]
    pub zid: String,
    #[prost(int32, tag = "2")]
    pub kind: i32,
    #[prost(double, tag = "3")]
    pub offset_seconds: f64,
    #[prost(double, tag = "4")]
    pub min_offset_seconds: f64,
    #[prost(double, tag = "5")]
    pub max_offset_seconds: f64,
    #[prost(double, tag = "6")]
    pub rejections_per_minute: f64,
    #[prost(uint64, tag = "7")]
    pub rejections_in_window: u64,
    #[prost(int64, tag = "8")]
    pub first_seen_unix_ms: i64,
    #[prost(int64, tag = "9")]
    pub last_seen_unix_ms: i64,
    #[prost(string, tag = "10")]
    pub observed_by: String,
    #[prost(string, optional, tag = "11")]
    pub attribution_name: Option<String>,
    #[prost(string, optional, tag = "12")]
    pub attribution_owner: Option<String>,
    #[prost(string, tag = "13")]
    pub grade: String,
    #[prost(string, tag = "14")]
    pub color: String,
    #[prost(double, tag = "15")]
    pub limit_seconds: f64,
    /// None = unknown (the reader had no sessions file).
    #[prost(bool, optional, tag = "16")]
    pub direct_session: Option<bool>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PeerFaultDetailV1 {
    #[prost(message, repeated, tag = "1")]
    pub peers: Vec<PeerFaultV1>,
    #[prost(string, tag = "2")]
    pub verdict: String,
    #[prost(string, tag = "3")]
    pub verdict_reason: String,
    #[prost(string, tag = "4")]
    pub local_clock: String,
    #[prost(string, tag = "5")]
    pub observed_by: String,
    #[prost(uint32, tag = "6")]
    pub peers_known: u32,
    #[prost(int64, tag = "7")]
    pub last_line_age_ms: i64,
    #[prost(uint64, tag = "8")]
    pub unparsed_lines_window: u64,
    #[prost(string, tag = "9")]
    pub peering: String,
    #[prost(double, tag = "10")]
    pub peering_rejections_per_minute: f64,
    #[prost(double, tag = "11")]
    pub peering_max_offset_seconds: f64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct EnvHealthV1 {
    #[prost(string, tag = "1")]
    pub domain: String,
    #[prost(string, tag = "2")]
    pub scope: String,
    #[prost(int64, tag = "3")]
    pub ts_unix_ms: i64,
    #[prost(string, tag = "4")]
    pub overall_color: String,
    #[prost(uint64, tag = "5")]
    pub sample_count: u64,
    #[prost(uint32, tag = "6")]
    pub window_secs: u32,
    #[prost(oneof = "EnvDetail", tags = "10, 11, 12, 13, 17")]
    pub detail: Option<EnvDetail>,
}

#[derive(Clone, PartialEq, prost::Oneof)]
pub enum EnvDetail {
    #[prost(message, tag = "10")]
    Fabric(FabricDetailStub),
    #[prost(message, tag = "11")]
    Time(TimeDetailV1),
    #[prost(message, tag = "12")]
    Mesh(MeshDetailV1),
    #[prost(message, tag = "13")]
    Presence(PresenceDetailV1),
    #[prost(message, tag = "17")]
    Peer(PeerFaultDetailV1),
}

/// Humanize an age in ms: "44s ago" / "2m ago" / "2h ago"; negative → "—".
pub fn fmt_age(ms: i64) -> String {
    if ms < 0 {
        return "—".to_string();
    }
    let s = ms / 1000;
    if s < 90 {
        format!("{s}s ago")
    } else if s < 5400 {
        format!("{}m ago", s / 60)
    } else {
        format!("{}h ago", s / 3600)
    }
}

/// Short display label: first DNS label of a peer fqdn.
pub fn short_name(fqdn: &str) -> &str {
    fqdn.split('.').next().unwrap_or(fqdn)
}
