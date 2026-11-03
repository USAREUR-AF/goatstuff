//! The merged-timeline event model. Every adapter normalizes its stream into
//! [`TimelineEvent`]s; the view layer merges them time-ordered. Honesty
//! discipline (ADR 1090): every event carries its source and its attribution
//! grade, and a source that produced nothing reports WHY via [`SourceStatus`]
//! — absence of events is never rendered as "no activity" unless the source
//! was actually read.

use serde::Serialize;

/// How confidently this event is attributed to the entity under the lens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribution {
    /// The source records the entity's own identifier (email, handle, anchor).
    Strong,
    /// Joined by naming convention only (e.g. NetBird peer-name handle prefix).
    Convention,
    /// The source's actor field is shared/coarse (e.g. portal `actor="operator"`).
    Coarse,
}

/// What KIND of thing an event is — did this entity CHANGE something, or
/// merely reach something?
///
/// The operator ask: "it's important to know when an entity is
/// taking some action, especially administrative, vs just surfing and
/// browsing." A timeline that renders `peer.group.delete` and `Ory login` with
/// identical weight makes the reader do that classification by eye, on every
/// row, from source-native action strings — which is exactly the work this
/// surface exists to do for them.
///
/// THE DEFAULT IS `Unclassified`, DELIBERATELY, AND IT MATTERS WHICH WAY.
/// Misreading a mutation as browsing is the dangerous error: it hides the
/// thing an investigator is looking for. Misreading browsing as a mutation
/// merely wastes a second look. So an action string this code does not
/// recognise is NEVER assumed passive — it renders as unclassified and asks
/// to be looked at. New source action codes appear without warning (NetBird
/// and Ory both add them across versions), so "unrecognised" is a normal
/// steady state, not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Changed identity, access, membership or configuration — the acts an
    /// administrator performs ON the system or ON another principal.
    Admin,
    /// Gained or exercised access: an authentication, a token grant, a session.
    /// Reaching a thing, not changing it.
    Access,
    /// Wrote data onto the fabric — an action, but on the data plane rather
    /// than the control plane, and worth separating from admin for that reason.
    Data,
    /// Recognised as observation only: a read, a listing, a presence sample.
    Passive,
    /// Not recognised. Never treated as harmless.
    Unclassified,
}

impl EventKind {
    /// Classify a source-native action string. Kept as ONE function so every
    /// adapter classifies identically and a new verb is added in one place.
    pub fn classify(action: &str) -> Self {
        let a = action.to_ascii_lowercase();
        // Control-plane mutation. Substring matching on purpose: sources spell
        // the same act differently (`peer.group.add`, `group_added`,
        // `vendor-delete`, `slot.identity.set`) and a closed list of exact
        // strings would silently drop the next spelling into Unclassified.
        const ADMIN: &[&str] = &[
            "add", "create", "delete", "remove", "revoke", "grant", "update", "set",
            "onboard", "enroll", "rotate", "assign", "approve", "reject", "disable",
            "enable", "promote", "demote", "reset", "transfer", "probation", "merge",
        ];
        const ACCESS: &[&str] = &["login", "logout", "session", "access_granted", "consent", "token", "auth"];
        const DATA: &[&str] = &["publish", "put", "write", "declare"];
        const PASSIVE: &[&str] = &["view", "list", "read", "get", "query", "last_seen", "heartbeat"];

        // An access NOUN can contain an admin VERB as a substring, and the
        // naive check got this backwards on the single most common row on the
        // page: `oauth2.access_granted` contains "grant", so every Hydra
        // consent grant classified as Admin. One operator carries 39 of them,
        // so a person merely reaching six services would have rendered as 39
        // administrative acts — the precise inversion this enum exists to
        // prevent, and the dangerous direction (it buries the real mutations
        // in noise).
        //
        // The rule that generalizes: strip the access nouns, THEN look for a
        // mutation verb in what remains. An admin verb that survives the strip
        // is genuinely operating ON an access object and stays Admin —
        // `consent.revoke` and `session.delete` are administrative acts
        // against someone's access and must not be softened to "access".
        let residue = ACCESS.iter().fold(a.clone(), |acc, noun| acc.replace(noun, "."));

        // Order matters: an action naming BOTH a mutation and a read verb is a
        // mutation. "get" inside "widget.delete" must not win.
        if ADMIN.iter().any(|k| residue.contains(k)) {
            return Self::Admin;
        }
        if DATA.iter().any(|k| a.contains(k)) {
            return Self::Data;
        }
        if ACCESS.iter().any(|k| a.contains(k)) {
            return Self::Access;
        }
        if PASSIVE.iter().any(|k| a.contains(k)) {
            return Self::Passive;
        }
        Self::Unclassified
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Access => "access",
            Self::Data => "data",
            Self::Passive => "passive",
            Self::Unclassified => "unclassified",
        }
    }
}

/// WHO the act was aimed at — did this entity DO it, or was it DONE TO them?
///
/// Orthogonal to [`EventKind`], deliberately. Kind classifies the verb; this
/// classifies the polarity. Collapsing them into one enum was the tempting
/// shortcut and it loses the distinction that matters most on an
/// investigative surface: an administrator resetting someone's groups and
/// that person changing their own both classify as `admin`, and they are not
/// remotely the same fact. Without this axis the timeline answers "what
/// happened around this entity" while claiming to answer "what did this
/// entity do".
///
/// The data was already there — `console-audit`, `portal-audit` and
/// `netbird-events` each compute actor-vs-target to decide whether a row
/// belongs to this entity at all, then threw the answer away into prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// The entity performed it on something or someone else.
    BySubject,
    /// Someone else performed it ON the entity. The entity is the object.
    AtSubject,
    /// The entity acted on ITSELF — self-service. Both actor and target.
    /// Worth its own value rather than folding into `BySubject`: "they
    /// changed their own groups" and "they changed someone else's" are
    /// different powers.
    Reflexive,
    /// The source records no actor/target polarity, or the row is an
    /// OBSERVATION rather than an act (a peer going offline is neither done
    /// by nor done to anyone). Never rendered as if it were `BySubject`.
    Undirected,
}

impl Direction {
    /// Derive from the two membership tests an adapter already performs.
    pub fn from_hits(as_actor: bool, as_target: bool) -> Self {
        match (as_actor, as_target) {
            (true, true) => Self::Reflexive,
            (true, false) => Self::BySubject,
            (false, true) => Self::AtSubject,
            (false, false) => Self::Undirected,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::BySubject => "by them",
            Self::AtSubject => "to them",
            Self::Reflexive => "self",
            Self::Undirected => "",
        }
    }
}

/// How an event ENDED, where the source records it.
///
/// Operator ask: surfacing failures turns this from a forensic
/// tool into a proactive usability watch — "we might see when certain events
/// fail and what the error was", so a broken flow is noticed from the timeline
/// rather than from someone complaining.
///
/// `Unrecorded` is not "it succeeded". Most sources here log only what
/// happened, not what was attempted and refused, and two layers refuse with no
/// record at all (zenoh-write F-177, the NetBird data plane). The silent-layers
/// banner already says so; this enum must not quietly contradict it by
/// rendering unrecorded outcomes as success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The source explicitly recorded success.
    Ok,
    /// The source explicitly recorded a REFUSAL — authorization said no. The
    /// most valuable row on the page: someone tried to do a thing they could
    /// not, which is either a permissions gap or an attempt worth seeing.
    Refused,
    /// The source explicitly recorded an ERROR — the system failed, not the
    /// policy. This is the usability signal: errors here mean the environment
    /// is broken for a real person right now.
    Failed,
    /// The source recorded no outcome. NOT a claim of success.
    Unrecorded,
}

impl Outcome {
    /// Classify a source-native result/status string.
    pub fn classify(result: &str) -> Self {
        let r = result.trim().to_ascii_lowercase();
        if r.is_empty() || r == "?" {
            return Self::Unrecorded;
        }
        if r.contains("refus") || r.contains("denied") || r.contains("deny")
            || r.contains("forbidden") || r.contains("unauthor") || r.contains("403")
        {
            return Self::Refused;
        }
        if r.contains("fail") || r.contains("error") || r.contains("timeout")
            || r.contains("invalid") || r.contains("reject")
        {
            return Self::Failed;
        }
        if r.contains("ok") || r.contains("success") || r.contains("granted") || r.contains("200") {
            return Self::Ok;
        }
        Self::Unrecorded
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::Unrecorded => "unrecorded",
        }
    }
}

/// HOW THE SAFE DEFAULTS ARE ENFORCED: they are not serde defaults.
///
/// `kind`, `direction` and `outcome` each carried a `#[serde(default = …)]`
/// pointing at their non-harmless value, which read like a guard and was
/// inert — this struct derives `Serialize` only, so those defaults could
/// never run. Removed rather than left as decoration, because a guard that
/// cannot fire is worse than none: it invites the next reader to trust it.
///
/// The real guarantee is stronger. Rust requires every field of a struct
/// literal to be given a value, so there is no way to construct a
/// `TimelineEvent` without stating all three explicitly. An adapter author
/// cannot forget one; the compiler refuses. Each enum also has no `Default`
/// impl, deliberately, so `..Default::default()` cannot smuggle one in.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineEvent {
    /// RFC3339 timestamp as recorded by the source — never re-stamped here.
    pub ts: String,
    /// Adapter name (matches `SourceStatus.source`).
    pub source: &'static str,
    /// Source-native action name (`create_identity`, `peer.login`, …).
    pub action: String,
    /// The actor identifier exactly as the source recorded it.
    pub actor: String,
    /// What the action was applied to, when the source distinguishes one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// One-line human summary.
    pub summary: String,
    /// Did this entity CHANGE something, or merely reach it? Derived from
    /// `action` via [`EventKind::classify`]; never guessed toward harmless.
    pub kind: EventKind,
    /// Did the entity DO this, or was it done TO them? Orthogonal to `kind`;
    /// see [`Direction`]. Defaults to `Undirected`, never to `BySubject` —
    /// crediting an entity with an act the source did not attribute to them
    /// is the one error this axis exists to prevent.
    pub direction: Direction,
    /// How it ended, where the source says so. `Unrecorded` never means "fine".
    pub outcome: Outcome,
    /// The source's own error/refusal text, verbatim, when it recorded one.
    /// This is the usability signal: a real message a real person hit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub attribution: Attribution,
    /// The source record, verbatim, for drill-down.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

/// Per-source read outcome. `skipped` = not configured on this deployment;
/// `error` = configured but the read failed. Both render honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    Ok,
    Skipped,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceStatus {
    pub source: &'static str,
    pub state: SourceState,
    /// Why skipped / what errored / where the data came from.
    pub detail: String,
    /// Events contributed to the timeline (0 is meaningful only when `state=ok`).
    pub n: usize,
}

#[cfg(test)]
mod tests {
    use super::{EventKind, Outcome};

    /// The collision that motivated the strip-then-match rule. `grant` is an
    /// admin verb and `access_granted` is an access noun that contains it; a
    /// plain substring test made every Hydra consent row administrative.
    #[test]
    fn access_nouns_do_not_absorb_admin_verbs() {
        assert_eq!(EventKind::classify("oauth2.access_granted"), EventKind::Access);
        assert_eq!(EventKind::classify("peer.login"), EventKind::Access);
        assert_eq!(EventKind::classify("session.refresh"), EventKind::Access);
    }

    /// The other direction, which must NOT be softened: an admin verb acting
    /// ON someone's access is administrative. Revoking a consent is not
    /// "someone reached a service".
    #[test]
    fn admin_verbs_on_access_objects_stay_admin() {
        assert_eq!(EventKind::classify("consent.revoke"), EventKind::Admin);
        assert_eq!(EventKind::classify("session.delete"), EventKind::Admin);
        assert_eq!(EventKind::classify("token.create"), EventKind::Admin);
    }

    #[test]
    fn mutations_outrank_read_verbs_in_the_same_string() {
        // "get" must not win inside a delete.
        assert_eq!(EventKind::classify("widget.get.delete"), EventKind::Admin);
        assert_eq!(EventKind::classify("group_added"), EventKind::Admin);
        assert_eq!(EventKind::classify("slot.identity.set"), EventKind::Admin);
    }

    #[test]
    fn passive_and_data_are_separated() {
        assert_eq!(EventKind::classify("peer.last_seen"), EventKind::Passive);
        assert_eq!(EventKind::classify("zenoh.publish"), EventKind::Data);
    }

    /// The default must never drift toward harmless: an unrecognised verb is
    /// surfaced for a look, not filed as browsing.
    #[test]
    fn unrecognised_actions_are_never_passive() {
        assert_eq!(EventKind::classify("frobnicate"), EventKind::Unclassified);
        assert_eq!(EventKind::classify(""), EventKind::Unclassified);
    }

    /// A refusal is policy saying no; a failure is the system breaking. They
    /// read differently to an operator and must not collapse together.
    #[test]
    fn refusal_and_failure_are_distinct() {
        assert_eq!(Outcome::classify("403 Forbidden"), Outcome::Refused);
        assert_eq!(Outcome::classify("permission denied"), Outcome::Refused);
        assert_eq!(Outcome::classify("connection timeout"), Outcome::Failed);
        assert_eq!(Outcome::classify("invalid_client"), Outcome::Failed);
    }

    /// The axis exists to separate "they did it" from "it was done to them".
    /// Folding these together is what made an admin resetting someone's
    /// groups indistinguishable from that person changing their own.
    #[test]
    fn polarity_is_derived_from_the_two_membership_tests() {
        use super::Direction as D;
        assert_eq!(D::from_hits(true, false), D::BySubject);
        assert_eq!(D::from_hits(false, true), D::AtSubject);
        assert_eq!(D::from_hits(true, true), D::Reflexive);
    }

    /// The dangerous default: a source that attributes nothing must never
    /// credit the entity with having acted.
    #[test]
    fn unattributed_never_reads_as_the_entity_acting() {
        use super::Direction as D;
        assert_eq!(D::from_hits(false, false), D::Undirected);
        assert_ne!(D::from_hits(false, false), D::BySubject);
        // Undirected renders as nothing at all rather than as a word a reader
        // could mistake for a claim.
        assert_eq!(D::Undirected.label(), "");
    }

    /// Silence is not success — the property the whole surface rests on.
    #[test]
    fn absent_outcome_is_never_ok() {
        assert_eq!(Outcome::classify(""), Outcome::Unrecorded);
        assert_eq!(Outcome::classify("?"), Outcome::Unrecorded);
        assert_ne!(Outcome::classify("   "), Outcome::Ok);
    }
}
