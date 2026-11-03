//! Shared request/response shapes + the persona/condition spectrum.
//!
//! The teaching frame is NOT "good user vs attacker" — that is a false
//! dichotomy. A principal is an *identity* plus an optional *condition*; most
//! conditions model ordinary failure (a stale credential, a noisy producer, a
//! service mid-replacement), and the hostile case is just one more condition.
//! Every gate fills a complementary role in one coherent strategy.

use serde::{Deserialize, Serialize};

/// One of the five gates. `na` = this gate does not participate in this kind of
/// attempt (e.g. SPIRE for a portal-minted org identity) — shown honestly, not
/// as a pass.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GateState {
    Pass,
    Fail,
    Na,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateVerdict {
    /// `netbird` | `cert` | `spire` | `ory` | `acl`
    pub gate: String,
    pub state: GateState,
    /// The complementary role this gate plays / the property at stake.
    pub role: String,
    /// What actually happened this attempt (live observation where possible).
    pub detail: String,
    /// Is this verdict a genuinely-live observation, or represented?
    pub live: bool,
}

/// The kind of principal an identity is — picks the issuance path (gate ②/③).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityKind {
    /// A human operator (role carried by the Ory JWT). Mints/administers.
    Operator,
    /// An external organization's system — portal/CA-minted leaf (gate ②).
    OrgSystem,
    /// A workload we run — SPIRE-attested SVID (gate ③).
    Workload,
    /// The disposable backing service for burn-&-respawn.
    Backing,
}

/// A condition applied to an attempt. The default is `fresh` (everything in
/// order). The rest span ordinary failure modes, not just malice.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Condition {
    /// Everything in order.
    #[default]
    Fresh,
    /// Not on the network (NetBird represented "off") — no L3 route.
    Unenrolled,
    /// The cryptographic credential has lapsed (a stale, not-revoked cert).
    Expired,
    /// Writing outside the identity's own lane/anchor.
    WrongLane,
    /// A noisy / overloaded producer — flooding, usually a bug not an attack.
    Noisy,
    /// Presenting an identity not rooted in any trusted issuer (the hostile case).
    Forged,
}

/// A demo identity in the roster. Anchors map 1:1 to the zenohd ACL CN grants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub handle: String,
    pub kind: IdentityKind,
    /// The write anchor (= the leaf CN the zenohd ACL keys on). None for a pure
    /// operator (whose authority is the JWT role, not a write anchor).
    pub anchor: Option<String>,
    /// Ory role for operator identities: ADMIN | OPERATOR | OBSERVER.
    pub role: Option<String>,
    /// URI-SAN grammar string minted into the leaf (the identity grammar shown).
    pub uri_san: Option<String>,
    /// Has this identity been revoked (mesh-first / represented)?
    pub revoked: bool,
    /// One-line "who is this" for the UI.
    pub blurb: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AttemptReq {
    pub identity: String,
    #[serde(default)]
    pub condition: Condition,
    /// Key to write. Defaults to `<anchor>/telemetry`.
    #[serde(default)]
    pub key: Option<String>,
    /// Payload. Defaults to a small JSON reading.
    #[serde(default)]
    pub payload: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttemptResp {
    pub identity: String,
    pub condition: Condition,
    pub key: String,
    pub gates: Vec<GateVerdict>,
    /// What the policy SHOULD do given identity ⊕ condition ⊕ key.
    pub expected: String,
    /// What was actually observed on the live mTLS subscriber.
    pub observed: String,
    /// The write was accepted at the handshake but dropped at the ACL with no
    /// error to the publisher — the silent-denial lesson.
    pub silently_denied: bool,
    /// Plain-language summary the UI shows.
    pub summary: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MintReq {
    /// The anchor to (re)issue a leaf for.
    pub anchor: String,
    /// rotate = re-mint a fresh leaf for the SAME anchor (ADR 1044), instant,
    /// no ACL change. Otherwise a first issuance.
    #[serde(default)]
    pub rotate: bool,
    /// The Ory ID-token JWT authenticating the human + role (must carry ADMIN to
    /// authorize the mint — the CA does the issuing, not Ory).
    pub jwt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MintResp {
    pub ok: bool,
    pub anchor: String,
    pub rotated: bool,
    pub cn: String,
    pub uri_san: String,
    pub serial: String,
    pub not_after: String,
    pub message: String,
}

/// A live event pushed to browsers over /ws so every viz is fed by real
/// orchestrator observation and never lies.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum WsEvent {
    /// A sample the real mTLS observer actually saw land on the fabric.
    Sample { key: String, payload: String, at: i64 },
    /// A gate verdict produced by a real attempt (drives the passive viz).
    Attempt { identity: String, summary: String, silently_denied: bool, at: i64 },
    /// An identity lifecycle change (mint / rotate / revoke).
    Lifecycle { action: String, anchor: String, at: i64 },
    /// The orchestrator's view of each backing layer's availability.
    Health { zenoh: bool, spire: bool, hydra: bool, at: i64 },
}
