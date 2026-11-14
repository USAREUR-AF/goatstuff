// SPDX-License-Identifier: Apache-2.0
//
// Wire types — keep field tags in lock-step with the canonical schema at
// ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto (arm 17). Only
// the `peer` arm is declared here; a decoder skips arms it does not know.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum PeerFaultKindV1 {
    PeerFaultKindUnspecified = 0,
    ClockAhead = 1,
    /// Clients refuse the router's certificate and this declared peer holds no
    /// session. A candidate, never a verdict.
    TrustRootRefusedCandidate = 2,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PeerFaultV1 {
    #[prost(string, tag = "1")]
    pub zid: String,
    #[prost(enumeration = "PeerFaultKindV1", tag = "2")]
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
    /// Whether this Zenoh ID holds a session with our router. Absent = unknown
    /// (no sessions file). A rejected timestamp names the router that STAMPED
    /// the sample, which can sit behind another router we do hold a session with.
    #[prost(bool, optional, tag = "16")]
    pub direct_session: Option<bool>,
    /// The peers-file key when it is a source address, not a Zenoh ID (a
    /// candidate row for such an entry has `zid` = "").
    #[prost(string, optional, tag = "17")]
    pub source_address: Option<String>,
    /// Candidate rows: the first reading at which this reader saw the declared
    /// peer hold no session. Since the reader started; not persisted.
    #[prost(int64, optional, tag = "18")]
    pub absent_since_unix_ms: Option<i64>,
}

/// One TLS alert name received in the window: a fact about the router.
#[derive(Clone, PartialEq, prost::Message)]
pub struct TlsAlertV1 {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(uint64, tag = "2")]
    pub count_in_window: u64,
    #[prost(double, tag = "3")]
    pub per_minute: f64,
    /// In the trust-refusal set (UnknownCA, BadCertificate, CertificateUnknown).
    #[prost(bool, tag = "4")]
    pub trust_refusal: bool,
    #[prost(int64, tag = "5")]
    pub last_seen_unix_ms: i64,
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
    /// Set when this reader sits beside one per-partner edge router: every
    /// rejection is attributed to this peering and `peers` is detail under it.
    #[prost(string, tag = "9")]
    pub peering: String,
    /// Peering roll-up: rejections per minute summed over all Zenoh IDs.
    #[prost(double, tag = "10")]
    pub peering_rejections_per_minute: f64,
    /// Peering roll-up: the largest offset seen in the window, seconds.
    #[prost(double, tag = "11")]
    pub peering_max_offset_seconds: f64,
    /// Trust detector, site level: every TLS alert name in the window, by name.
    #[prost(message, repeated, tag = "12")]
    pub tls_alerts: Vec<TlsAlertV1>,
    /// The trust detector alone: clean|unattributed|candidates|unmeasured.
    #[prost(string, tag = "13")]
    pub trust_verdict: String,
    /// Declared peers holding no session; absent = cannot be judged.
    #[prost(uint32, optional, tag = "14")]
    pub declared_peers_absent: Option<u32>,
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
    #[prost(oneof = "EnvDetail", tags = "17")]
    pub detail: Option<EnvDetail>,
}

#[derive(Clone, PartialEq, prost::Oneof)]
pub enum EnvDetail {
    #[prost(message, tag = "17")]
    Peer(PeerFaultDetailV1),
}
