// SPDX-License-Identifier: Apache-2.0
//
// goat-fabric-crawler — the SINGLE lazy crawler
// (docs/design/zenoh-cardinality-budget-and-lazy-crawler.md §4, layer 4).
//
// The reproduced wedge is the
// multiplication of three factors on the router's one routing RwLock:
// keyexpr cardinality × broad concurrent crawling × active declaration churn.
// This service removes the crawl factor structurally and bounds the read cost
// of measuring the cardinality factor:
//
//   - ONE crawler, sequential per-owner queries — never N concurrent walkers,
//     never one giant `**` walk. Caps read-hold CONCURRENCY (one brief holder).
//   - LAZY per-owner enumeration — stops consuming at the sprawl threshold
//     (default 100, aligned with the budget's exception tier) and emits a
//     summary {owner, stored_keys_observed >= threshold, truncated: true}
//     instead of walking all N. Caps read-hold DURATION on the abusers.
//   - Everyone else reads the published rollup (ops/fabric/cardinality/<site>)
//     instead of the router — the poll-once/fan-out pattern.
//   - Doubles as the cardinality-budget SENSOR: per-owner counts + budget_state
//     are the reconciliation input for the tiered topic-budget grant (layer 2).
//     Reconciliation, not interception (ADR 1025) — this flags, it never blocks.
//
// Honesty discipline (environment-health-surface.md §6 applied to a census):
//   - a truncated owner is reported as ">= observed", never a fabricated total;
//   - an owner skipped by the crawl wall-budget is reported skipped, never 0;
//   - the crawler's own cost (per-owner ms, total crawl ms) is on the surface —
//     the instrument's observer-cost is visible, not hidden
//     (zenoh-wedge-instrumentation.md).
//
// Zero replies for an owner is honest "nothing stored / not granted", never
// proof of denial.
//
// Router-independence ( fragility audit, item 7): in rocksdb-direct
// mode the census and the budget classification never touch the router, so a
// failed zenoh session does not stop the crawler. It runs the census, retries
// the session once per cycle, and marks the facets that DO need the router —
// adminspace declarations, the scoped-GET baseline, publication of the rollup
// and census mirror — unavailable while they are missing. The budget sensor
// must not die with the router it measures, and it must not paper over what
// it could not see.

use anyhow::{Context, Result};
use clap::Parser;
use prost::Message as _;
use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use zenoh::qos::{CongestionControl, Priority};

// ---------------------------------------------------------------------------
// Wire types — keep field tags in lock-step with
// proto/fabric_cardinality_v1.proto (prost-derive, no protoc; precedent:
// crypto-posture / time-health).
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, prost::Message)]
struct OwnerCardinalityV1 {
    /// First keyexpr chunk — the tenant anchor / namespace owner
    /// (ADR 1025 structure-in-the-namespace).
    #[prost(string, tag = "1")]
    owner: String,
    /// Distinct stored keys observed under `<owner>/**` this cycle. When
    /// `truncated`, this is a LOWER BOUND (enumeration stopped at the lazy
    /// threshold / iteration cap), never a total.
    #[prost(uint64, tag = "2")]
    stored_keys_observed: u64,
    /// Enumeration stopped at the lazy threshold — the owner sprawls past it.
    #[prost(bool, tag = "3")]
    truncated: bool,
    /// This owner was not queried this cycle (crawl wall-budget exhausted).
    /// stored_keys_observed is meaningless when set — honest "not measured".
    #[prost(bool, tag = "4")]
    skipped: bool,
    /// Wall time of this owner's query (read-hold self-cost, visible).
    #[prost(uint32, tag = "5")]
    query_ms: u32,
    /// Budget tier the observed count lands in: "self" (<= self tier),
    /// "request" (<= request tier), "exception" (past request tier — the
    /// lazy-threshold trip IS the budget violation flag), "unmeasured" when
    /// skipped. Tiers per the cardinality-budget design §2.
    #[prost(string, tag = "6")]
    budget_state: String,
    /// Where this row's count came from: "rocksdb-exact" (secondary read of
    /// the live store, distinct keys in one column family — exact),
    /// "rocksdb-capped" (iteration hit the cap; lower bound + estimate),
    /// "zenoh-walk" (bounded query fallback — lower bound when truncated).
    #[prost(string, tag = "7")]
    source: String,
    /// rocksdb `estimate-num-keys` O(1) ceiling (over-counts dead entries);
    /// 0 when not read via rocksdb.
    #[prost(uint64, tag = "8")]
    estimate: u64,
    /// Granted allowance from the topic-budget allocation (layer 2b): the
    /// per-tenant agreed distinct-key ceiling resolved from the budget
    /// source. 0 = NO GRANT ON RECORD (budget_state renders "unmeasured"),
    /// NEVER "allowance zero, everything is a violation" — the honesty rule
    /// governing this surface. Also 0 when no budget source is configured
    /// (legacy global-tier classification stays in force).
    #[prost(uint64, tag = "9")]
    granted: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
struct FabricCardinalityV1 {
    /// Site scope — the `<site>` segment of ops/fabric/cardinality/<site>.
    #[prost(string, tag = "1")]
    site: String,
    #[prost(int64, tag = "2")]
    sampled_at_ms: i64,
    /// Per-owner census, deterministic order (owner asc).
    #[prost(message, repeated, tag = "3")]
    owners: Vec<OwnerCardinalityV1>,
    /// Sum of stored_keys_observed — a LOWER BOUND on stored keys (and stored
    /// keys are themselves a lower bound on routing edges; the authoritative
    /// edge count needs the router declare log, wedge-instrumentation producer 3).
    #[prost(uint64, tag = "4")]
    stored_keys_observed_total: u64,
    #[prost(uint32, tag = "5")]
    owners_truncated: u32,
    #[prost(uint32, tag = "6")]
    owners_skipped: u32,
    /// Admin census declaration count (wildcard-collapsed; the 847-class).
    #[prost(uint64, tag = "7")]
    admin_declarations: u64,
    /// Wall time of the admin census queries (read-hold cost trend, H1 analog).
    #[prost(uint32, tag = "8")]
    admin_census_ms: u32,
    /// Cheap scoped GET latency (contention baseline; the crawler's own rollup
    /// key, granted by the universal read rule).
    #[prost(uint32, tag = "9")]
    scoped_get_ms: u32,
    /// Total crawl wall time this cycle (observer-cost, visible).
    #[prost(uint32, tag = "10")]
    crawl_ms: u32,
    /// The lazy sprawl threshold in force (aligned with the budget exception tier).
    #[prost(uint64, tag = "11")]
    lazy_threshold: u64,
    /// |delta observed keys| / interval — NET-change proxy only (equal
    /// declares+undeclares are invisible; weaker still under truncation).
    #[prost(double, tag = "12")]
    declare_rate_proxy_per_s: f64,
    #[prost(int64, tag = "13")]
    proxy_interval_s: i64,
    /// Health of the topic-budget allowance source this cycle (layer 2b):
    /// "none" (no source configured — legacy global-tier classification),
    /// "live" (fetched this cycle), "stale" (fetch failed; classifying
    /// against the last good map — grants may lag), "unavailable" (never
    /// fetched; every owner reads unmeasured). Surfaced so a dead source
    /// cannot silently blank the exception alert (the F-177 failure class,
    /// applied to the instrument itself).
    #[prost(string, tag = "14")]
    budget_source_state: String,
    /// Health of the crawler's own zenoh session, measured as TRANSPORT
    /// reachability — is a router actually connected right now — never as
    /// "do we hold a Session object" (F-280: the object outlives the
    /// transport, so object-existence reported "live" through a four-minute
    /// router outage).
    ///
    /// EXACTLY TWO VALUES, and this is a load-bearing constraint:
    ///   "live"        — a router is connected; the router-dependent facets
    ///                   below were measured this cycle;
    ///   "unavailable" — no router reachable; `admin_declarations`,
    ///                   `admin_census_ms` and `scoped_get_ms` are
    ///                   placeholders, NOT observations (the band JSON
    ///                   renders them null), and the rollup + census mirror
    ///                   were not published.
    ///
    /// Do NOT add a third value that means "facets are stale/partial".
    /// Consumers map `!= "unavailable"` to trustworthy (see
    /// ops/relay/health/zenoh-health-to-prom.py), deliberately so that an
    /// unknown future state cannot silently start paging — which also means
    /// a new not-really-live state would silently be read as trustworthy.
    /// The invariant to preserve: `state != "unavailable"` ⟺ the three facets
    /// in this same message are real observations. Triage detail belongs in
    /// `zenoh_session_detail`, which nothing gates on.
    #[prost(string, tag = "15")]
    zenoh_session_state: String,
    /// Why the session is in that state — triage only, NOTHING gates on it:
    /// "connected" (a router transport is up), "no-transport" (the Session
    /// object is alive but every transport is down — zenoh is reconnecting
    /// underneath; the router died or the link broke), "no-session" (open
    /// never succeeded — look at certs/ACL/endpoint before the router).
    /// Answers the first question an operator asks when the degraded alert
    /// fires: is this the router, or is it me?
    #[prost(string, tag = "16")]
    zenoh_session_detail: String,
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested).
// ---------------------------------------------------------------------------

/// Extract the namespace owner (first data-plane keyexpr chunk) from an admin
/// census reply key `@/<zid>/router/{publisher,subscriber}/<keyexpr...>`.
/// Returns None for admin-space declarations (`@/...`), wildcard first chunks
/// (`*`/`$*`-ish — nothing to bucket), and malformed keys.
fn owner_from_census_key(key: &str) -> Option<String> {
    let rest = key
        .split_once("/router/publisher/")
        .or_else(|| key.split_once("/router/subscriber/"))
        .map(|(_, rest)| rest)?;
    let first = rest.split('/').next()?;
    if first.is_empty() || first.starts_with('@') || first.contains('*') {
        return None;
    }
    Some(first.to_string())
}

/// Budget tier for an observed count against the GLOBAL default tiers — the
/// awareness-only classification used when no budget source is configured.
/// A truncated owner is past the request tier by construction (threshold
/// trip = the violation flag).
fn budget_state(
    observed: u64,
    truncated: bool,
    skipped: bool,
    self_tier: u64,
    request_tier: u64,
) -> &'static str {
    if skipped {
        "unmeasured"
    } else if truncated || observed > request_tier {
        "exception"
    } else if observed > self_tier {
        "request"
    } else {
        "self"
    }
}

/// Budget tier for an observed count against a per-tenant GRANTED allowance —
/// the allocation-half classification (layer 2b). Semantics:
///   - no grant on record (None or 0) => "unmeasured" — never "allowance
///     zero, everything is a violation";
///   - observed > granted => "exception" — over what was agreed. This holds
///     for truncated rows too: the lower bound already exceeds the grant;
///   - within grant => the label names the APPROVAL TIER of the allowance
///     ("self" self-service block, "request" sponsor-approved, "granted"
///     platform-admin exception-tier allowance), so the tier series answers
///     "who agreed to this" rather than re-bucketing the observed count.
///     A truncated-but-within-grant row keeps its truncated flag visible
///     (count-is-a-floor honesty rides that bit, not this label).
fn budget_state_granted(
    observed: u64,
    skipped: bool,
    self_tier: u64,
    request_tier: u64,
    granted: Option<u64>,
) -> &'static str {
    if skipped {
        return "unmeasured";
    }
    let Some(g) = granted.filter(|g| *g > 0) else {
        return "unmeasured";
    };
    if observed > g {
        "exception"
    } else if g <= self_tier {
        "self"
    } else if g <= request_tier {
        "request"
    } else {
        "granted"
    }
}

// ---------------------------------------------------------------------------
// Topic-budget allowance source (layer 2b — the allocation half).
// ---------------------------------------------------------------------------

/// Per-tenant granted allowances, loaded from the budget-source JSON
/// projection. Registry-projection interim (same pattern + caveat as ADR 1101's
/// active-anchor list): the SoT is the per-goatnet group_vars grant record,
/// rendered to this file by the fabric-crawler role; the registry grant store
/// (ADR 1075 — live in the portal since the data-access build) is the named
/// follow-up home, at which point the source becomes the resolver endpoint.
/// The env var is scheme-open (FABRIC_CRAWLER_BUDGET_SOURCE) on purpose.
struct BudgetMap {
    /// The automatic self-service allowance (the starting block) every
    /// onboarded slot holds by construction — allocation design §2 tier 1.
    self_service_block: u64,
    /// Onboarded slot anchors carrying the automatic block.
    auto: BTreeSet<String>,
    /// Explicit grants (sponsor / platform-admin approved): owner -> granted.
    /// Overrides the automatic block when present.
    grants: std::collections::BTreeMap<String, u64>,
}

/// The "configured but never loaded" map: no grants on record, so every owner
/// classifies unmeasured — honest, and never a false violation.
static EMPTY_BUDGET: BudgetMap = BudgetMap {
    self_service_block: 0,
    auto: BTreeSet::new(),
    grants: std::collections::BTreeMap::new(),
};

impl BudgetMap {
    /// The allowance on record for an owner, or None (no grant — unmeasured).
    fn granted_for(&self, owner: &str) -> Option<u64> {
        self.grants
            .get(owner)
            .copied()
            .or_else(|| self.auto.contains(owner).then_some(self.self_service_block))
            .filter(|g| *g > 0)
    }

    fn parse(raw: &[u8]) -> Result<BudgetMap> {
        let v: serde_json::Value =
            serde_json::from_slice(raw).context("budget source: parse JSON")?;
        let self_service_block = v
            .get("self_service_block")
            .and_then(|x| x.as_u64())
            .context("budget source: self_service_block missing/not a u64")?;
        let auto = v
            .get("auto_self_service_anchors")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str())
                    .map(String::from)
                    .collect::<BTreeSet<String>>()
            })
            .unwrap_or_default();
        let mut grants = std::collections::BTreeMap::new();
        if let Some(obj) = v.get("grants").and_then(|x| x.as_object()) {
            for (owner, entry) in obj {
                let g = entry
                    .get("granted")
                    .and_then(|x| x.as_u64())
                    .with_context(|| {
                        format!("budget source: grants[{owner}].granted missing/not a u64")
                    })?;
                grants.insert(owner.clone(), g);
            }
        }
        Ok(BudgetMap {
            self_service_block,
            auto,
            grants,
        })
    }
}

/// Classify one owner row: (budget_state, granted-on-the-wire). With a budget
/// map the state is grant-relative (allocation); without one it is the legacy
/// global-tier awareness classification and granted rides as 0.
fn classify_owner(
    owner: &str,
    observed: u64,
    truncated: bool,
    skipped: bool,
    self_tier: u64,
    request_tier: u64,
    budget: Option<&BudgetMap>,
) -> (&'static str, u64) {
    match budget {
        None => (
            budget_state(observed, truncated, skipped, self_tier, request_tier),
            0,
        ),
        Some(b) => {
            let g = b.granted_for(owner);
            (
                budget_state_granted(observed, skipped, self_tier, request_tier, g),
                g.unwrap_or(0),
            )
        }
    }
}

/// `(zenoh_session_state, zenoh_session_detail)` from what we actually know:
/// whether a Session object is held, and whether a transport is reachable.
///
/// The load-bearing row is (true, false) — object alive, transport gone. That
/// is what a router outage looks like from inside a running crawler, and
/// reporting it as "live" was F-280. Reachability alone decides the state;
/// the object only colours the triage detail.
fn session_state(has_session: bool, reachable: bool) -> (&'static str, &'static str) {
    match (has_session, reachable) {
        (_, true) => ("live", "connected"),
        (true, false) => ("unavailable", "no-transport"),
        (false, false) => ("unavailable", "no-session"),
    }
}

/// An honest "not measured this cycle" row: the count is meaningless, the
/// state is unmeasured (never a tier claim), and `source` names WHY —
/// "zenoh-walk" (crawl wall-budget exhausted) or "zenoh-unavailable" (no
/// session, and no rocksdb source to fall back to).
fn unmeasured_row(
    owner: &str,
    source: &str,
    self_tier: u64,
    request_tier: u64,
    budget: Option<&BudgetMap>,
) -> OwnerCardinalityV1 {
    let (state, granted) = classify_owner(owner, 0, false, true, self_tier, request_tier, budget);
    OwnerCardinalityV1 {
        owner: owner.to_string(),
        stored_keys_observed: 0,
        truncated: false,
        skipped: true,
        query_ms: 0,
        budget_state: state.to_string(),
        source: source.to_string(),
        estimate: 0,
        granted,
    }
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "goat-fabric-crawler",
    about = "Single lazy crawler — bounded per-owner cardinality census + rollup (cardinality-budget layer 4)"
)]
struct Args {
    /// Zenoh router endpoint, e.g. `tls/goat-zenoh-router:7447`.
    #[arg(long, env = "FABRIC_CRAWLER_ENDPOINT")]
    endpoint: String,

    /// Site identifier — the `<site>` segment of ops/fabric/cardinality/<site>.
    #[arg(long, env = "FABRIC_CRAWLER_SITE")]
    site: String,

    /// Leaf X.509 cert (PEM) for mTLS. Required for a `tls/` endpoint.
    #[arg(long, env = "FABRIC_CRAWLER_CERT")]
    cert: Option<String>,
    /// Matching private key (PEM).
    #[arg(long, env = "FABRIC_CRAWLER_KEY_FILE")]
    key: Option<String>,
    /// CA root cert (PEM) used to verify the router.
    #[arg(long, env = "FABRIC_CRAWLER_ROOT_CA")]
    root_ca: Option<String>,

    /// Crawl interval (ms). Single-flight by construction (sequential loop).
    #[arg(long, env = "FABRIC_CRAWLER_INTERVAL_MS", default_value_t = 60_000)]
    interval_ms: u64,

    /// Lazy sprawl threshold: per-owner enumeration stops here and reports a
    /// truncated summary. Aligned with the budget exception tier (>100).
    #[arg(long, env = "FABRIC_CRAWLER_LAZY_THRESHOLD", default_value_t = 100)]
    lazy_threshold: u64,

    /// Budget self-service tier (the automatic starting block; design §2,
    /// revised from 10) — labeling only.
    #[arg(long, env = "FABRIC_CRAWLER_SELF_TIER", default_value_t = 100)]
    self_tier: u64,
    /// Budget request tier (sponsor-approved ceiling; revised
    /// from 100) — labeling only.
    #[arg(long, env = "FABRIC_CRAWLER_REQUEST_TIER", default_value_t = 1_000)]
    request_tier: u64,

    /// Pause between per-owner queries (ms) — lets writers take the routing
    /// lock between our reads instead of facing sustained read pressure.
    /// Rig-measured: 256 owners at 50 ms paused = 13.5 s cycle with
    /// scoped GETs flat at baseline; 100 ms keeps a 256-store site inside a
    /// 60 s interval with margin.
    #[arg(long, env = "FABRIC_CRAWLER_OWNER_PAUSE_MS", default_value_t = 100)]
    owner_pause_ms: u64,

    /// Total crawl wall-budget per cycle (ms). Owners past it are reported
    /// skipped (honest "not measured"), picked up first next cycle.
    #[arg(long, env = "FABRIC_CRAWLER_CRAWL_BUDGET_MS", default_value_t = 20_000)]
    crawl_budget_ms: u64,

    /// Per-query timeout (ms).
    #[arg(
        long,
        env = "FABRIC_CRAWLER_QUERY_TIMEOUT_MS",
        default_value_t = 10_000
    )]
    query_timeout_ms: u64,

    /// Optional path to also write the rollup as JSON — the router-health
    /// band's zenoh-fabric.json contract (superset of the retired shell
    /// sampler's fields), bind-mounted to the relay-health ingest dir.
    #[arg(long, env = "FABRIC_CRAWLER_JSON_OUT")]
    json_out: Option<String>,

    /// Top-N owners in the JSON `top_owners_by_stored_keys` field.
    #[arg(long, env = "FABRIC_CRAWLER_TOPN", default_value_t = 8)]
    topn: usize,

    /// Extra owner prefixes to always crawl (comma-separated), on top of the
    /// admin-census discovery + owners remembered from prior cycles.
    #[arg(long, env = "FABRIC_CRAWLER_SEED_OWNERS", default_value = "ops")]
    seed_owners: String,

    /// PREFERRED census source on router hosts: the
    /// router's rocksdb storage base dir (one store dir per owner, ADR 1101),
    /// mounted read-only. Each store is opened as a rocksdb SECONDARY
    /// (read-only follower of the live primary — never takes its lock) and
    /// distinct keys are counted in ONE column family (the backend keeps 2 —
    /// payload + data_info — so naive all-CF counts double, rig-verified).
    /// Zero router involvement. When unset/unreadable, falls back to the
    /// bounded zenoh walk.
    #[arg(long, env = "FABRIC_CRAWLER_ROCKSDB_DIR")]
    rocksdb_dir: Option<String>,

    /// Writable scratch dir for the rocksdb secondaries' catch-up logs.
    #[arg(
        long,
        env = "FABRIC_CRAWLER_ROCKSDB_SECONDARY_DIR",
        default_value = "/tmp/fabric-crawler-secondary"
    )]
    rocksdb_secondary_dir: String,

    /// Exact-iteration cap per store: keys-only iteration stops here and the
    /// row reports a lower bound + the O(1) estimate (source rocksdb-capped).
    /// Rig-measured: full exact iteration of 256 stores incl. a 51.8k-key
    /// sprawler took 797 ms, so the cap is a pathology guard, not a tuning.
    #[arg(
        long,
        env = "FABRIC_CRAWLER_ROCKSDB_EXACT_CAP",
        default_value_t = 1_000_000
    )]
    rocksdb_exact_cap: u64,

    /// Topic-budget allowance source (layer 2b): a file path OR an http(s)
    /// URL serving the budget JSON projection ({self_service_block,
    /// auto_self_service_anchors, grants}). An http(s) source is the portal's
    /// /api/budget-projection resolver endpoint (the ADR 1075 registry grant
    /// store); a path is the ansible-rendered projection-file interim.
    /// Re-read every cycle, so a grant change lands without a crawler restart;
    /// a fetch failure classifies against the LAST GOOD map (stale) or, never
    /// having loaded, reads every owner unmeasured (unavailable) — never a
    /// fallback to the legacy global tiers.
    #[arg(long, env = "FABRIC_CRAWLER_BUDGET_SOURCE")]
    budget_source: Option<String>,

    /// Bearer token for an http(s) budget source (the portal operator key).
    /// Ignored for file sources.
    #[arg(long, env = "FABRIC_CRAWLER_BUDGET_TOKEN")]
    budget_token: Option<String>,

    /// Container-healthcheck probe: read the heartbeat this crawler writes,
    /// exit 0 if a cycle completed inside the freshness window it stamped,
    /// non-zero otherwise. Intercepted BEFORE argument parsing (the probe must
    /// not need `--endpoint`/`--site`), so the heartbeat path comes from the
    /// environment alone — see `heartbeat_path`. The runtime image is
    /// distroless: this flag exists because there is no shell, curl or wget in
    /// it to probe with, and inventing an HTTP endpoint for a probe would be a
    /// bigger surface than a file mtime.
    ///
    /// LIVENESS ONLY. A crawler running the rocksdb-direct census with a dead
    /// zenoh session is HEALTHY — router state belongs on the data surface
    /// (`zenoh_session_state`), never on container health, or docker would
    /// re-couple the crawler to the router that the census deliberately does
    /// not need.
    #[arg(long)]
    healthcheck: bool,

    /// Cycles the heartbeat may go unwritten before the probe calls it stale.
    /// The window is stamped INTO the heartbeat by the daemon, so the probe
    /// never re-derives it and the two cannot drift.
    #[arg(
        long,
        env = "FABRIC_CRAWLER_HEARTBEAT_MAX_INTERVALS",
        default_value_t = 3
    )]
    heartbeat_max_intervals: u32,
}

// ---------------------------------------------------------------------------
// Heartbeat + the --healthcheck probe (container liveness).
// ---------------------------------------------------------------------------

/// Where the heartbeat lives. Resolved from the environment ONLY, because the
/// daemon and the probe are two processes that share nothing but the container
/// env — a CLI flag would be set on one and missed by the other. Order:
///   1. `FABRIC_CRAWLER_HEARTBEAT_FILE` — explicit;
///   2. a sibling of `FABRIC_CRAWLER_JSON_OUT` — that dir is already a
///      writable bind mount wherever the band JSON is configured;
///   3. `/tmp/fabric-crawler-heartbeat.json`.
/// An explicit path equal to the band JSON is REFUSED (rule 2 is used
/// instead): the two writers would share a `.tmp` name and the heartbeat
/// would silently overwrite the band surface the router-health band reads.
/// Refusing in this one function keeps daemon and probe in agreement.
fn heartbeat_path(explicit: Option<&str>, json_out: Option<&str>) -> std::path::PathBuf {
    let explicit = explicit.filter(|p| !p.is_empty() && Some(*p) != json_out);
    if let Some(p) = explicit {
        return std::path::PathBuf::from(p);
    }
    if let Some(dir) = json_out
        .filter(|p| !p.is_empty())
        .map(std::path::Path::new)
        .and_then(|p| p.parent())
        .filter(|d| !d.as_os_str().is_empty())
    {
        return dir.join("fabric-crawler-heartbeat.json");
    }
    std::path::PathBuf::from("/tmp/fabric-crawler-heartbeat.json")
}

fn heartbeat_path_from_env() -> std::path::PathBuf {
    heartbeat_path(
        std::env::var("FABRIC_CRAWLER_HEARTBEAT_FILE")
            .ok()
            .as_deref(),
        std::env::var("FABRIC_CRAWLER_JSON_OUT").ok().as_deref(),
    )
}

/// The heartbeat document. Self-describing on purpose: it carries the instant
/// past which it should be considered stale, so the probe compares two numbers
/// and no freshness threshold is duplicated between the daemon, the probe and
/// the compose template (change the interval, the window follows).
fn heartbeat_doc(
    now_ms: i64,
    healthy_for_ms: i64,
    phase: &str,
    cycles_completed: u64,
    last_cycle_ms: u32,
    r: Option<&FabricCardinalityV1>,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "fabric-crawler-heartbeat/v1",
        "written_at_ms": now_ms,
        "healthy_until_ms": now_ms + healthy_for_ms,
        "phase": phase,
        "cycles_completed": cycles_completed,
        "last_cycle_ms": last_cycle_ms,
        // Reported for the operator reading `docker inspect`, NOT an input to
        // the verdict — a degraded session is still a live crawler.
        "zenoh_session_state": r.map(|r| r.zenoh_session_state.as_str()).unwrap_or(""),
        "zenoh_session_detail": r.map(|r| r.zenoh_session_detail.as_str()).unwrap_or(""),
        "budget_source_state": r.map(|r| r.budget_source_state.as_str()).unwrap_or(""),
        "owners": r.map(|r| r.owners.len()).unwrap_or(0),
    })
}

/// The probe's verdict: (healthy, one-line reason). `doc` is None when the
/// heartbeat is missing or unparseable — never written, or the process died
/// before its first write.
fn healthcheck_verdict(doc: Option<&serde_json::Value>, now_ms: i64, path: &str) -> (bool, String) {
    let Some(d) = doc else {
        return (
            false,
            format!("UNHEALTHY: no readable heartbeat at {path} (never written, or the path is not writable by this container)"),
        );
    };
    let Some(until) = d.get("healthy_until_ms").and_then(|v| v.as_i64()) else {
        return (
            false,
            format!("UNHEALTHY: heartbeat at {path} has no healthy_until_ms (wrong schema)"),
        );
    };
    let written = d.get("written_at_ms").and_then(|v| v.as_i64()).unwrap_or(0);
    let age_s = (now_ms - written).max(0) / 1000;
    let phase = d.get("phase").and_then(|v| v.as_str()).unwrap_or("?");
    // Empty at startup — the heartbeat is armed before the first cycle has
    // measured anything. Render that as unknown rather than as a blank.
    let state = d
        .get("zenoh_session_state")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("?");
    // Qualify a non-connected state with WHY, so the one line an operator
    // sees in `docker inspect` distinguishes "the router went away" from
    // "this crawler never got a session at all". The state itself is now
    // transport-truthful, so "zenoh_session=live" here means a router
    // is genuinely connected rather than merely that a Session object exists.
    let detail = d
        .get("zenoh_session_detail")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let session = if detail.is_empty() || detail == "connected" {
        state.to_string()
    } else {
        format!("{state}/{detail}")
    };
    let cycles = d
        .get("cycles_completed")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if now_ms <= until {
        (
            true,
            format!(
                "healthy: {phase} {age_s}s ago (cycles={cycles}, zenoh_session={session} — router state does not gate this verdict)"
            ),
        )
    } else {
        (
            false,
            format!(
                "UNHEALTHY: heartbeat {age_s}s old, stale {}s past its window (last phase {phase}, cycles={cycles})",
                (now_ms - until) / 1000
            ),
        )
    }
}

/// Write the heartbeat atomically (tmp + rename), same as the band JSON, so a
/// probe can never read a half-written document. A failure is LOUD: an
/// unwritable heartbeat path would otherwise mark a perfectly healthy
/// container unhealthy forever with no stated cause.
fn write_heartbeat(path: &std::path::Path, doc: &serde_json::Value) {
    let tmp = path.with_extension("json.tmp");
    let wrote = std::fs::write(&tmp, doc.to_string()).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = wrote {
        warn!(
            error = %e,
            path = path.to_string_lossy().as_ref(),
            "heartbeat write FAILED — the crawler is fine but --healthcheck will report unhealthy; point FABRIC_CRAWLER_HEARTBEAT_FILE at a writable path"
        );
    }
}

/// Run the probe and return the process exit code. Prints one line — docker
/// keeps healthcheck output, so `docker inspect` shows the reason.
fn run_healthcheck() -> i32 {
    let path = heartbeat_path_from_env();
    let doc = std::fs::read(&path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok());
    let (healthy, reason) =
        healthcheck_verdict(doc.as_ref(), now_unix_ms(), &path.to_string_lossy());
    println!("goat-fabric-crawler {reason}");
    i32::from(!healthy)
}

/// The rocksdb census base when the mode is actually usable — configured AND
/// readable as a directory. `None` means every census this run has to go
/// through the router (the bounded zenoh walk).
fn rocksdb_base(args: &Args) -> Option<std::path::PathBuf> {
    args.rocksdb_dir
        .as_ref()
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
}

/// Whether a zenoh session is REQUIRED for this run to measure anything at
/// all. With a usable rocksdb base the census and the budget classification
/// are router-independent, so a session failure degrades
/// facets instead of killing the sensor — the budget sensor must not die with
/// the router it measures. Without one, the router IS the only census source
/// and a session failure stays fatal (fail fast, not fail silent).
fn session_required(rocksdb_base: Option<&std::path::Path>) -> bool {
    rocksdb_base.is_none()
}

/// Read the budget source — http(s) GET (portal resolver endpoint) or file.
/// Bounded: 5 s timeout, 1 MiB response cap (a projection is a few KB).
fn read_budget_source(src: &str, bearer: Option<&str>) -> Result<Vec<u8>> {
    if src.starts_with("http://") || src.starts_with("https://") {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(5))
            .build();
        let mut req = agent.get(src);
        if let Some(t) = bearer {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        let resp = req.call().context("budget source: http fetch")?;
        let mut buf = Vec::new();
        use std::io::Read;
        resp.into_reader()
            .take(1 << 20)
            .read_to_end(&mut buf)
            .context("budget source: read body")?;
        Ok(buf)
    } else {
        std::fs::read(src).context("budget source: read file")
    }
}

// The adminspace selectors — copied from panoscope-core census. Two hard-won
// constraints (do not "simplify" these):
//   1. Zenoh admin keyspace is QUERY-only in 1.x: declare_subscriber("@/**")
//      returns Ok and receives zero samples (verified live). Poll
//      with session.get().
//   2. The broad `@/<router_zid>/**` query is DENIED by the least-privilege
//      census ACL: zenoh authorization allows a query only when an allow rule
//      *includes* it (`nodes_including`), and `**` cannot cross the verbatim
//      `@` chunk. Query the EXACT granted selectors (`trial-peer-r-admin-census`:
//      `@/*/router/{subscriber,publisher}/**`) — the broad query returned zero
//      census, silently,.
const CENSUS_SELECTORS: [&str; 2] = ["@/*/router/subscriber/**", "@/*/router/publisher/**"];

async fn open_session(args: &Args) -> Result<zenoh::Session> {
    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("config mode: {e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{}\"]", args.endpoint))
        .map_err(|e| anyhow::anyhow!("config connect/endpoints: {e}"))?;
    // No multicast scouting — we connect to exactly the configured router.
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .map_err(|e| anyhow::anyhow!("config scouting: {e}"))?;
    if args.endpoint.starts_with("tls/") {
        let cert = args
            .cert
            .as_ref()
            .context("--cert required for tls/ endpoint")?;
        let key = args
            .key
            .as_ref()
            .context("--key required for tls/ endpoint")?;
        let root_ca = args
            .root_ca
            .as_ref()
            .context("--root-ca required for tls/ endpoint")?;
        let tls_block = format!(
            r#"{{ "root_ca_certificate": "{root_ca}", "connect_certificate": "{cert}", "connect_private_key": "{key}", "enable_mtls": true, "verify_name_on_connect": false }}"#
        );
        config
            .insert_json5("transport/link/tls", &tls_block)
            .map_err(|e| anyhow::anyhow!("config transport/link/tls: {e}"))?;
    }
    zenoh::open(config)
        .await
        .map_err(|e| anyhow::anyhow!("zenoh::open failed: {e}"))
}

/// Is a router (or peer) ACTUALLY reachable through this session right now?
///
/// F-280: holding a `zenoh::Session` proves nothing about the transport. When
/// the router dies under a running crawler, zenoh keeps the Session object
/// alive and retries the link underneath (`zenoh_link_tls` retry warnings),
/// and every operation keeps returning `Ok` — `put` accepts into a session
/// with nowhere to send, `get` yields zero replies, which this crawler is
/// required to read as honest emptiness rather than failure. So
/// neither object existence nor operation return values can detect the
/// outage; the transport table can.
///
/// `Session::info().routers_zid()` resolves to the runtime's list of
/// CURRENTLY OPEN unicast transports filtered by whatami (zenoh 1.9.0
/// `runtime::get_zids` → `manager().get_transports_unicast()`). It is a local
/// lookup — no query, no network round-trip, so it adds nothing to the
/// router's read-hold and cannot make this instrument the wedge it measures.
/// Peers are checked too so the predicate is honest if this is ever run in
/// peer mode; today the config is hardcoded client mode.
async fn router_reachable(session: &zenoh::Session) -> bool {
    if session.info().routers_zid().await.next().is_some() {
        return true;
    }
    session.info().peers_zid().await.next().is_some()
}

/// One adminspace census reply, relayed byte-faithfully. The mirror publishes
/// these so census consumers (panoscope-core::census — panoscope daemon 5s,
/// `goat top` 5s, `goat who` 2s today) can swap their own adminspace GET for a
/// rollup subscription with ZERO parse changes: `key` is the original admin
/// reply key (`@/<zid>/router/{publisher,subscriber}/<keyexpr>`) and `payload`
/// the original reply body (the JSON naming the source peers).
#[derive(serde::Serialize)]
struct CensusEntry {
    key: String,
    payload: serde_json::Value,
}

/// One admin census pass: owners discovered from declared keyexprs + the
/// wildcard-collapsed declaration count + the byte-faithful entry mirror +
/// the wall time.
async fn admin_census(
    session: &zenoh::Session,
    timeout: Duration,
) -> (BTreeSet<String>, Vec<CensusEntry>, u32) {
    let t0 = Instant::now();
    let mut owners = BTreeSet::new();
    let mut entries: Vec<CensusEntry> = Vec::new();
    for selector in CENSUS_SELECTORS {
        let replies = match session.get(selector).timeout(timeout).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, selector, "admin census query failed");
                continue;
            }
        };
        while let Ok(reply) = replies.recv_async().await {
            let Ok(sample) = reply.result() else { continue };
            let key = sample.key_expr().as_str().to_string();
            if let Some(owner) = owner_from_census_key(&key) {
                owners.insert(owner);
            }
            let payload = serde_json::from_slice::<serde_json::Value>(&sample.payload().to_bytes())
                .unwrap_or(serde_json::Value::Null);
            entries.push(CensusEntry { key, payload });
        }
    }
    entries.sort_by(|a, b| a.key.cmp(&b.key)); // deterministic ordering
    (owners, entries, t0.elapsed().as_millis() as u32)
}

/// Bounded per-owner enumeration: count distinct keys under `<owner>/**`,
/// stopping (and closing the query) at the lazy threshold. Deduped because
/// replicated/overlapping storages may reply for the same key; the dedup set
/// is bounded by the threshold, so memory is O(threshold), not O(N).
async fn crawl_owner(
    session: &zenoh::Session,
    owner: &str,
    lazy_threshold: u64,
    timeout: Duration,
) -> (u64, bool, u32) {
    let t0 = Instant::now();
    let selector = format!("{owner}/**");
    let replies = match session.get(&selector).timeout(timeout).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, owner, "owner query failed");
            return (0, false, t0.elapsed().as_millis() as u32);
        }
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut truncated = false;
    while let Ok(reply) = replies.recv_async().await {
        let Ok(sample) = reply.result() else { continue };
        if !truncated {
            seen.insert(sample.key_expr().as_str().to_string());
            if seen.len() as u64 >= lazy_threshold {
                // LAZY: stop counting — the summary replaces the walk.
                truncated = true;
            }
        }
        // Past the threshold we keep DRAINING without storing: zenoh 1.9 has
        // no query cancellation on receiver-drop, so the server sends every
        // reply regardless — dropping the receiver just spams `sending on a
        // closed channel` errors (observed on the rig). Draining
        // costs the same wire traffic, keeps memory O(threshold), and the
        // sprawler's full reply cost stays VISIBLE in query_ms (honesty rule:
        // observer cost on the surface). The true duration cap for sprawlers
        // is the rocksdb-direct source (design doc §"Layer-4 implementation"),
        // which never issues the query at all.
    }
    (
        seen.len() as u64,
        truncated,
        t0.elapsed().as_millis() as u32,
    )
}

/// rocksdb-direct census of ONE live store. Secondary
/// read-only open — never takes the primary's (the running router's) lock.
/// Counts distinct keys in a single column family: zenoh-backend-rocksdb
/// keeps 2 CFs (payload + data_info), one entry per key each, so counting
/// one CF equals the consolidated key count (rig cross-validated to the key
/// against REST). `estimate-num-keys` is an O(1) ceiling that over-counts
/// dead/un-compacted entries — reported alongside, never as the count.
fn rocksdb_store_census(
    primary: &std::path::Path,
    secondary: &std::path::Path,
    exact_cap: u64,
) -> Result<(u64, u64, bool)> {
    // NOTE: keep_log_file_num is deliberately NOT set here. It does not work
    // for secondary instances — RocksDB skips its info-LOG purge on that path,
    // which is why the live dirs held ~21,500 files each despite the library
    // default of 1000, and why setting it to 2 explicitly still left 24 files
    // across a 25-open loop. The debris is cleaned by
    // reap_secondary_log_rotations below instead.
    let opts = rocksdb::Options::default();
    let cfs = rocksdb::DB::list_cf(&opts, primary).map_err(|e| anyhow::anyhow!("list_cf: {e}"))?;
    let db = rocksdb::DB::open_cf_as_secondary(&opts, primary, secondary, &cfs)
        .map_err(|e| anyhow::anyhow!("open_as_secondary: {e}"))?;
    let _ = db.try_catch_up_with_primary();
    // Count in the first CF ("default" — the payload CF). All CFs hold one
    // entry per key; one CF = the key count.
    let cf_name = cfs
        .iter()
        .find(|c| c.as_str() == "default")
        .or_else(|| cfs.first())
        .context("store has no column families")?;
    let cf = db.cf_handle(cf_name).context("cf handle")?;
    let estimate = db
        .property_int_value_cf(&cf, "rocksdb.estimate-num-keys")
        .ok()
        .flatten()
        .unwrap_or(0);
    let mut exact: u64 = 0;
    let mut capped = false;
    let mut it = db.raw_iterator_cf(&cf);
    it.seek_to_first();
    while it.valid() {
        exact += 1;
        if exact >= exact_cap {
            capped = true;
            break;
        }
        it.next();
    }
    drop(it);
    drop(db);
    reap_secondary_log_rotations(secondary);
    Ok((exact, estimate, capped))
}

/// Delete the `LOG.old.<ts>` files RocksDB leaves behind in a secondary dir.
///
/// RocksDB renames `LOG` -> `LOG.old.<ts>` on EVERY open, and this crawler
/// opens one secondary per store per cycle — so each store dir gains a ~37 KB
/// file every `interval_ms`, forever, until the tree is nothing but LOG.old files
/// measured in hundreds of GiB.
///
/// `keep_log_file_num` does NOT fix this: RocksDB does not run its info-LOG
/// purge for secondary instances. Proven, not assumed — with
/// `set_keep_log_file_num(2)` a 25-open loop still left 24 files, which is why
/// the crawler reaps its own debris here instead. The accompanying test locks
/// that in.
///
/// Best-effort by design: a census that produced a correct count must not fail
/// because a cleanup unlink lost a race. Errors are ignored, and the host-side
/// `fabric-crawler-logprune.timer` remains as the backstop for hosts running an
/// older image.
fn reap_secondary_log_rotations(secondary: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(secondary) else {
        return;
    };
    for entry in entries.flatten() {
        // Strictly LOG.old.* — never LOG, CURRENT, MANIFEST-*, OPTIONS-* or
        // *.sst, which are live database state.
        if entry.file_name().to_string_lossy().starts_with("LOG.old.") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Census every store dir under the rocksdb base (dir name = owner anchor
/// under ADR 1101's store-per-live-slot layout; `ops`/`attest_schema` etc.
/// are operator-infra stores). Overlapping infra stores (e.g.
/// `ops_audit_operator` ⊂ `ops`) are reported as their own rows — per-store
/// truth, no double-count folding. Returns rows keyed by store/dir name.
fn rocksdb_census(
    base: &std::path::Path,
    secondary_base: &std::path::Path,
    exact_cap: u64,
    self_tier: u64,
    request_tier: u64,
    budget: Option<&BudgetMap>,
) -> Vec<OwnerCardinalityV1> {
    let mut rows = Vec::new();
    let Ok(entries) = std::fs::read_dir(base) else {
        return rows;
    };
    let mut dirs: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let t0 = Instant::now();
        match rocksdb_store_census(&dir, &secondary_base.join(&name), exact_cap) {
            Ok((exact, estimate, capped)) => {
                let (state, granted) =
                    classify_owner(&name, exact, capped, false, self_tier, request_tier, budget);
                rows.push(OwnerCardinalityV1 {
                    owner: name,
                    stored_keys_observed: exact,
                    truncated: capped,
                    skipped: false,
                    query_ms: t0.elapsed().as_millis() as u32,
                    budget_state: state.to_string(),
                    source: if capped {
                        "rocksdb-capped"
                    } else {
                        "rocksdb-exact"
                    }
                    .to_string(),
                    estimate,
                    granted,
                });
            }
            Err(e) => {
                warn!(error = %e, store = name.as_str(), "rocksdb store census failed; row skipped (honest: not measured)");
                let (_, granted) =
                    classify_owner(&name, 0, false, true, self_tier, request_tier, budget);
                rows.push(OwnerCardinalityV1 {
                    owner: name,
                    stored_keys_observed: 0,
                    truncated: false,
                    skipped: true,
                    query_ms: t0.elapsed().as_millis() as u32,
                    budget_state: "unmeasured".to_string(),
                    source: "rocksdb-error".to_string(),
                    estimate: 0,
                    granted,
                });
            }
        }
    }
    rows
}

/// Band-compatible JSON: the zenoh-fabric.json contract the router-health
/// band and Prometheus bridge already consume — a superset of the retired
/// shell sampler's fields, names kept stable on purpose.
fn band_json(r: &FabricCardinalityV1, topn: usize) -> serde_json::Value {
    let mut top: Vec<&OwnerCardinalityV1> = r.owners.iter().filter(|o| !o.skipped).collect();
    top.sort_by(|a, b| {
        b.stored_keys_observed
            .cmp(&a.stored_keys_observed)
            .then(a.owner.cmp(&b.owner))
    });
    let top_owners: Vec<serde_json::Value> = top
        .iter()
        .take(topn)
        .map(|o| {
            serde_json::json!({
                "owner": o.owner,
                "stored_keys": o.stored_keys_observed,
                "truncated": o.truncated,
                "budget_state": o.budget_state,
                "source": o.source,
                "estimate": o.estimate,
                "granted": o.granted,
            })
        })
        .collect();
    // Full per-owner budget attribution (layer 2b) — every non-skipped owner,
    // not just the top-N, so "every owner with a grant" is answerable from
    // the bridge's metrics (the verify criterion). Emitted only when an
    // allocation source is in force; in legacy mode the top-N rows above
    // remain the (awareness-only) budget surface.
    let owners_budget: Option<Vec<serde_json::Value>> =
        (r.budget_source_state != "none" && !r.budget_source_state.is_empty()).then(|| {
            r.owners
                .iter()
                .filter(|o| !o.skipped)
                .map(|o| {
                    serde_json::json!({
                        "owner": o.owner,
                        "granted": o.granted,
                        "budget_state": o.budget_state,
                        "stored_keys": o.stored_keys_observed,
                        "truncated": o.truncated,
                    })
                })
                .collect()
        });
    // Router-dependent facets: measured only with a live zenoh session. With
    // none, they render null (absent series downstream) rather than zero —
    // "zero declarations" is a confident lie about a fabric nobody looked at.
    // An empty state is an old publisher that always had a session.
    let session_live = r.zenoh_session_state != "unavailable";
    let facet = |v: serde_json::Value| -> serde_json::Value {
        if session_live {
            v
        } else {
            serde_json::Value::Null
        }
    };
    serde_json::json!({
        "router": r.site,
        "source": "goat-fabric-crawler",
        "sampled_at": r.sampled_at_ms / 1000,
        "stored_keys_total": r.stored_keys_observed_total,
        "top_owners_by_stored_keys": top_owners,
        "admin_declarations": facet(r.admin_declarations.into()),
        "admin_walk_ms": facet(r.admin_census_ms.into()),
        "scoped_get_ms": facet(r.scoped_get_ms.into()),
        "zenoh_session_state": if r.zenoh_session_state.is_empty() { "live" } else { r.zenoh_session_state.as_str() },
        // Triage only — no consumer gates on this (see the proto contract).
        "zenoh_session_detail": if r.zenoh_session_detail.is_empty() { serde_json::Value::Null } else { serde_json::json!(r.zenoh_session_detail) },
        "declare_rate_proxy_per_s": if r.proxy_interval_s > 0 { serde_json::json!(r.declare_rate_proxy_per_s) } else { serde_json::Value::Null },
        "proxy_interval_s": if r.proxy_interval_s > 0 { serde_json::json!(r.proxy_interval_s) } else { serde_json::Value::Null },
        "owners_total": r.owners.len(),
        "owners_truncated": r.owners_truncated,
        "owners_skipped": r.owners_skipped,
        "lazy_threshold": r.lazy_threshold,
        "crawl_ms": r.crawl_ms,
        "budget_source_state": if r.budget_source_state.is_empty() { "none" } else { r.budget_source_state.as_str() },
        "owners_budget": owners_budget,
        "note": if session_live {
            "single lazy crawler: per-owner counts are LOWER BOUNDS when truncated; admin_walk_ms = the granted census selectors (not a full @ walk); stored keys are a lower bound on routing edges"
        } else {
            "single lazy crawler, DEGRADED: no zenoh session this cycle — the rocksdb-direct census and budget classification are router-independent and still ran; admin_declarations / admin_walk_ms / scoped_get_ms were NOT measured (null, not zero) and the rollup + census mirror were NOT published"
        },
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    // The probe runs before argument parsing: a docker healthcheck invokes the
    // binary with no arguments at all, and `--endpoint`/`--site` are required.
    // Everything it needs is in the container environment.
    if std::env::args().any(|a| a == "--healthcheck") {
        std::process::exit(run_healthcheck());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let rollup_key = format!("ops/fabric/cardinality/{}", args.site);
    let census_key = format!("ops/fabric/census/{}", args.site);

    // The session is OPTIONAL in rocksdb-direct mode: the census and the
    // budget classification never touch the router, so a dead router must not
    // take the budget sensor down with it. Reconnect is attempted every cycle;
    // until it succeeds the router-dependent facets read unavailable.
    let mut session: Option<zenoh::Session> = match open_session(&args).await {
        Ok(s) => Some(s),
        Err(e) => {
            if session_required(rocksdb_base(&args).as_deref()) {
                return Err(e).context("open zenoh session");
            }
            warn!(
                error = %e,
                endpoint = args.endpoint.as_str(),
                "zenoh session unavailable at startup; continuing rocksdb-direct — census + rollup publication NOT running (facets marked unavailable), retrying each cycle"
            );
            None
        }
    };
    info!(
        zid = session
            .as_ref()
            .map(|s| s.zid().to_string())
            .unwrap_or_else(|| "-".to_string())
            .as_str(),
        endpoint = args.endpoint.as_str(),
        site = args.site.as_str(),
        publishes = rollup_key.as_str(),
        lazy_threshold = args.lazy_threshold,
        // Whether the OBJECT opened. Deliberately not called "live": that
        // word is reserved for transport reachability, re-measured every
        // cycle, because conflating the two is exactly F-280.
        session_object = session.is_some(),
        "single lazy crawler starting"
    );

    let query_timeout = Duration::from_millis(args.query_timeout_ms);
    let crawl_budget = Duration::from_millis(args.crawl_budget_ms);
    let owner_pause = Duration::from_millis(args.owner_pause_ms);

    // Owners remembered across cycles (an owner that stops declaring but still
    // has stored keys must keep being counted). In-memory only — rediscovered
    // from the census + storage on restart.
    let mut known_owners: BTreeSet<String> = args
        .seed_owners
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    // Skipped-first fairness: owners the wall-budget cut off last cycle go
    // first this cycle, so a big fleet can't starve the tail forever.
    let mut crawl_first: Vec<String> = Vec::new();
    let mut prev_total: Option<(i64, u64)> = None; // (unix_s, observed_total)
                                                   // Last good budget map — kept across a failed re-read so a transient
                                                   // source outage degrades to "stale" (grants may lag), never to a silent
                                                   // reversion of every tier.
    let mut budget_map: Option<BudgetMap> = None;

    // Container liveness (see --healthcheck). The window covers the configured
    // number of intervals plus one full crawl budget, so a legitimately slow
    // cycle never reads as death. Written once now — a fresh container is
    // healthy before its first crawl lands — then after every cycle.
    let hb_path = heartbeat_path_from_env();
    let hb_window_ms = (args.interval_ms as i64) * (args.heartbeat_max_intervals.max(1) as i64)
        + args.crawl_budget_ms as i64;
    let mut cycles_completed: u64 = 0;
    write_heartbeat(
        &hb_path,
        &heartbeat_doc(now_unix_ms(), hb_window_ms, "starting", 0, 0, None),
    );
    info!(
        path = hb_path.to_string_lossy().as_ref(),
        window_ms = hb_window_ms,
        "heartbeat armed (--healthcheck reads this; liveness only, router state never gates it)"
    );

    let mut ticker = tokio::time::interval(Duration::from_millis(args.interval_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("install SIGTERM handler")?;

    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = sigterm.recv() => { info!("SIGTERM received; shutting down"); break; }
            _ = tokio::signal::ctrl_c() => { info!("SIGINT received; shutting down"); break; }
        }

        // Reconnect attempt BEFORE the cycle clock starts: a stalled connect
        // must not eat the crawl wall-budget and skip owners that the
        // rocksdb-direct source could have measured. Bounded by the query
        // timeout so a black-holed endpoint costs one timeout, not the cycle.
        if session.is_none() {
            let t0 = Instant::now();
            match tokio::time::timeout(query_timeout, open_session(&args)).await {
                Ok(Ok(s)) => {
                    info!(zid = %s.zid(), reconnect_ms = t0.elapsed().as_millis() as u64,
                          "zenoh session re-established; census + publication facets live again");
                    session = Some(s);
                }
                Ok(Err(e)) => warn!(error = %e, reconnect_ms = t0.elapsed().as_millis() as u64,
                                    "zenoh session still unavailable; rocksdb-direct census continues, router facets unmeasured"),
                Err(_) => warn!(reconnect_ms = t0.elapsed().as_millis() as u64,
                                "zenoh session reconnect timed out; rocksdb-direct census continues, router facets unmeasured"),
            }
        }

        let cycle_t0 = Instant::now();

        // 0. Topic-budget allowance source (layer 2b): re-read every cycle so
        //    a grant change lands without a crawler restart. The projection
        //    file is the registry-projection interim (see BudgetMap).
        let budget_source_state: &str = match args.budget_source.as_deref() {
            None => "none",
            Some(path) => match read_budget_source(path, args.budget_token.as_deref())
                .and_then(|raw| BudgetMap::parse(&raw))
            {
                Ok(m) => {
                    budget_map = Some(m);
                    "live"
                }
                Err(e) => {
                    if budget_map.is_some() {
                        warn!(error = %e, path, "budget source re-read failed; classifying against the LAST GOOD map (stale)");
                        "stale"
                    } else {
                        warn!(error = %e, path, "budget source unreadable and never loaded; every owner reads unmeasured");
                        "unavailable"
                    }
                }
            },
        };
        // In force only when a source is configured: an unavailable source
        // yields an empty map (=> every owner unmeasured, honest), never a
        // fallback to the legacy global tiers (which would misread "big" as
        // "violation" the moment an allocation is supposed to exist).
        let budget_in_force: Option<&BudgetMap> = if budget_source_state == "none" {
            None
        } else if budget_source_state == "unavailable" {
            Some(&EMPTY_BUDGET)
        } else {
            budget_map.as_ref()
        };

        // 1 + 2. The ROUTER-DEPENDENT facets, measured only with a session:
        //    the admin census (owner discovery + declaration count + the
        //    byte-faithful mirror + read-hold trend) and the scoped-GET
        //    baseline (our own rollup key, granted by the universal read
        //    rule; zero replies on the first cycle is fine — the routing
        //    round-trip is the measurement, not the payload).
        //    Without a session these are NOT measured: the zeros below ride
        //    with zenoh_session_state="unavailable" and render null in the
        //    band JSON, never a fabricated "zero declarations".
        //    Gated on TRANSPORT reachability, not on holding a Session: the
        //    object survives a router outage and every call keeps returning
        //    Ok, so asking the object whether it exists answers the wrong
        //    question. With no transport the queries would return
        //    zero replies after burning two full query timeouts, and the zero
        //    would render as a confident "no declarations".
        let reachable = match session.as_ref() {
            Some(s) => router_reachable(s).await,
            None => false,
        };
        let (zenoh_session_state, zenoh_session_detail) =
            session_state(session.is_some(), reachable);
        let (census_entries, admin_census_ms, scoped_get_ms) = match session.as_ref() {
            Some(s) if reachable => {
                let (census_owners, entries, census_ms) = admin_census(s, query_timeout).await;
                known_owners.extend(census_owners);
                let scoped_t0 = Instant::now();
                if let Ok(replies) = s.get(rollup_key.as_str()).timeout(query_timeout).await {
                    while let Ok(_reply) = replies.recv_async().await {}
                }
                (entries, census_ms, scoped_t0.elapsed().as_millis() as u32)
            }
            _ => (Vec::new(), 0, 0),
        };
        let admin_declarations = census_entries.len() as u64;
        if session.is_some() && !reachable {
            warn!(
                endpoint = args.endpoint.as_str(),
                "zenoh session held but NO router transport — zenoh is reconnecting underneath; router facets unmeasured this cycle, rocksdb-direct census continues"
            );
        }

        // 3. The storage census. PREFERRED: rocksdb-direct (zero router
        //    involvement, exact counts — M-091 scenario C). FALLBACK: the
        //    sequential bounded zenoh walk (skipped-first, then abc order),
        //    which needs a session. With neither, every owner is reported
        //    unmeasured rather than dropped.
        let mut rows: Vec<OwnerCardinalityV1>;
        if args.rocksdb_dir.is_some() && rocksdb_base(&args).is_none() {
            warn!(
                dir = args.rocksdb_dir.as_deref().unwrap_or(""),
                "rocksdb dir configured but not readable"
            );
        }
        if let Some(base) = rocksdb_base(&args) {
            let secondary = std::path::PathBuf::from(&args.rocksdb_secondary_dir);
            let _ = std::fs::create_dir_all(&secondary);
            rows = rocksdb_census(
                &base,
                &secondary,
                args.rocksdb_exact_cap,
                args.self_tier,
                args.request_tier,
                budget_in_force,
            );
        } else if let Some(s) = session.as_ref().filter(|_| reachable) {
            // Reachability gates the walk too, and for a sharper reason than
            // wasted timeouts: a query on a dead transport returns zero
            // replies, and zero replies are read here as honest "nothing
            // stored". Walking without a transport would therefore
            // report every owner as 0 stored keys — a fabricated census, not
            // a missing one.
            let mut order: Vec<String> = Vec::with_capacity(known_owners.len());
            for o in crawl_first.drain(..) {
                if known_owners.contains(&o) {
                    order.push(o);
                }
            }
            for o in &known_owners {
                if !order.contains(o) {
                    order.push(o.clone());
                }
            }

            rows = Vec::with_capacity(order.len());
            for (i, owner) in order.iter().enumerate() {
                if cycle_t0.elapsed() >= crawl_budget {
                    // Honest "not measured this cycle"; front of the line next cycle.
                    for o in &order[i..] {
                        crawl_first.push(o.clone());
                        rows.push(unmeasured_row(
                            o,
                            "zenoh-walk",
                            args.self_tier,
                            args.request_tier,
                            budget_in_force,
                        ));
                    }
                    break;
                }
                let (observed, truncated, query_ms) =
                    crawl_owner(s, owner, args.lazy_threshold, query_timeout).await;
                let (state, granted) = classify_owner(
                    owner,
                    observed,
                    truncated,
                    false,
                    args.self_tier,
                    args.request_tier,
                    budget_in_force,
                );
                rows.push(OwnerCardinalityV1 {
                    owner: owner.clone(),
                    stored_keys_observed: observed,
                    truncated,
                    skipped: false,
                    query_ms,
                    budget_state: state.to_string(),
                    source: "zenoh-walk".to_string(),
                    estimate: 0,
                    granted,
                });
                tokio::time::sleep(owner_pause).await;
            }
        } else {
            // Neither census source this cycle: no readable rocksdb mount AND
            // no usable transport for the walk. Every remembered owner is
            // reported unmeasured — an empty or all-zero census would read as
            // "the fabric is empty", the exact confident lie this surface
            // must not tell.
            warn!(
                owners = known_owners.len(),
                detail = zenoh_session_detail,
                "no readable rocksdb dir and no reachable router; every owner reported unmeasured this cycle"
            );
            rows = known_owners
                .iter()
                .map(|o| {
                    unmeasured_row(
                        o,
                        "zenoh-unavailable",
                        args.self_tier,
                        args.request_tier,
                        budget_in_force,
                    )
                })
                .collect();
        }
        rows.sort_by(|a, b| a.owner.cmp(&b.owner)); // deterministic ordering
        let total: u64 = rows
            .iter()
            .filter(|r| !r.skipped)
            .map(|r| r.stored_keys_observed)
            .sum();
        let truncated_n: u32 = rows.iter().filter(|r| r.truncated).count() as u32;
        let skipped_n: u32 = rows.iter().filter(|r| r.skipped).count() as u32;

        // 4. Churn proxy from the previous cycle's total.
        let now_ms = now_unix_ms();
        let now_s = now_ms / 1000;
        let (proxy, proxy_interval_s) = match prev_total {
            Some((prev_s, prev_n)) if now_s > prev_s => {
                let elapsed = now_s - prev_s;
                ((total.abs_diff(prev_n)) as f64 / elapsed as f64, elapsed)
            }
            _ => (0.0, 0),
        };
        prev_total = Some((now_s, total));

        let rollup = FabricCardinalityV1 {
            site: args.site.clone(),
            sampled_at_ms: now_ms,
            owners: rows,
            stored_keys_observed_total: total,
            owners_truncated: truncated_n,
            owners_skipped: skipped_n,
            admin_declarations,
            admin_census_ms,
            scoped_get_ms,
            crawl_ms: cycle_t0.elapsed().as_millis() as u32,
            lazy_threshold: args.lazy_threshold,
            declare_rate_proxy_per_s: proxy,
            proxy_interval_s,
            budget_source_state: budget_source_state.to_string(),
            zenoh_session_state: zenoh_session_state.to_string(),
            zenoh_session_detail: zenoh_session_detail.to_string(),
        };

        // 5. Publish the rollup + the census mirror — both need the router.
        //    Gated on reachability, not on holding a Session: `put` on a
        //    transport-less session returns Ok and the payload goes nowhere,
        //    so publishing anyway would log "published cardinality rollup"
        //    for a write that never left the box. Without a transport the
        //    cycle's work still lands in the band JSON (step 6) and the
        //    missing write is logged as missing.
        if let Some(s) = session.as_ref().filter(|_| reachable) {
            // C0: Block + high priority — the census must not be starved by
            // the load it reports on.
            let payload = rollup.encode_to_vec();
            match s
                .put(&rollup_key, payload)
                .congestion_control(CongestionControl::Block)
                .priority(Priority::InteractiveHigh)
                .await
            {
                Ok(()) => info!(
                    key = rollup_key.as_str(),
                    owners = rollup.owners.len(),
                    total = total,
                    truncated = truncated_n,
                    skipped = skipped_n,
                    crawl_ms = rollup.crawl_ms,
                    "published cardinality rollup"
                ),
                Err(e) => warn!(error = %e, "publish rollup failed"),
            }

            // 5b. The census mirror (JSON — byte-faithful adminspace relay;
            //     consumers keep their existing parse). This is what lets
            //     panoscope / `goat top` / `goat who` stop walking the
            //     adminspace themselves: one crawler polls once, everyone
            //     fans out.
            let census_doc = serde_json::json!({
                "site": args.site,
                "sampled_at_ms": now_ms,
                "selectors": CENSUS_SELECTORS,
                "entries": census_entries,
            });
            match s
                .put(&census_key, census_doc.to_string())
                .congestion_control(CongestionControl::Block)
                .priority(Priority::InteractiveHigh)
                .await
            {
                Ok(()) => info!(
                    key = census_key.as_str(),
                    entries = admin_declarations,
                    "published census mirror"
                ),
                Err(e) => warn!(error = %e, "publish census mirror failed"),
            }
        } else {
            warn!(
                owners = rollup.owners.len(),
                total = total,
                crawl_ms = rollup.crawl_ms,
                band_json = args.json_out.is_some(),
                detail = zenoh_session_detail,
                "no reachable router: census measured but rollup + census mirror NOT published (router-dependent facets unavailable)"
            );
        }

        // 6. Optional band JSON (zenoh-fabric.json contract).
        if let Some(path) = &args.json_out {
            let json = band_json(&rollup, args.topn);
            let tmp = format!("{path}.tmp");
            let write =
                std::fs::write(&tmp, json.to_string()).and_then(|()| std::fs::rename(&tmp, path));
            if let Err(e) = write {
                warn!(error = %e, path = path.as_str(), "write band json failed");
            }
        }

        // 7. Heartbeat — the cycle completed, whatever the router was doing.
        cycles_completed += 1;
        write_heartbeat(
            &hb_path,
            &heartbeat_doc(
                now_unix_ms(),
                hb_window_ms,
                "cycle-complete",
                cycles_completed,
                rollup.crawl_ms,
                Some(&rollup),
            ),
        );
    }

    if let Some(s) = session {
        let _ = s.close().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_extraction_from_census_keys() {
        assert_eq!(
            owner_from_census_key("@/abc123/router/publisher/a2dc1592/video/frame"),
            Some("a2dc1592".to_string())
        );
        assert_eq!(
            owner_from_census_key("@/abc123/router/subscriber/ops/fabric/health/zagros"),
            Some("ops".to_string())
        );
        // Wildcard first chunk — nothing to bucket.
        assert_eq!(owner_from_census_key("@/abc123/router/subscriber/**"), None);
        assert_eq!(
            owner_from_census_key("@/abc123/router/subscriber/*/x"),
            None
        );
        // Admin-space declaration — not a data owner.
        assert_eq!(
            owner_from_census_key("@/abc123/router/publisher/@/local/thing"),
            None
        );
        // Malformed / unrelated admin keys.
        assert_eq!(owner_from_census_key("@/abc123/router/config/acl"), None);
        assert_eq!(owner_from_census_key("ops/fabric/health/zagros"), None);
    }

    #[test]
    fn budget_state_tiers_are_honest() {
        // Within self-service.
        assert_eq!(budget_state(3, false, false, 10, 100), "self");
        assert_eq!(budget_state(10, false, false, 10, 100), "self");
        // Request tier.
        assert_eq!(budget_state(11, false, false, 10, 100), "request");
        assert_eq!(budget_state(100, false, false, 10, 100), "request");
        // Past request = exception, and a truncation IS the violation flag
        // regardless of the observed count.
        assert_eq!(budget_state(101, false, false, 10, 100), "exception");
        assert_eq!(budget_state(100, true, false, 10, 100), "exception");
        // Skipped is unmeasured, never a tier claim.
        assert_eq!(budget_state(0, false, true, 10, 100), "unmeasured");
    }

    #[test]
    fn band_json_keeps_sampler_contract_fields() {
        let r = FabricCardinalityV1 {
            site: "zagros".into(),
            sampled_at_ms: 1_700_000_000_000,
            owners: vec![
                OwnerCardinalityV1 {
                    owner: "a2dc1592".into(),
                    stored_keys_observed: 100,
                    truncated: true,
                    skipped: false,
                    query_ms: 42,
                    budget_state: "exception".into(),
                    source: "zenoh-walk".into(),
                    estimate: 0,
                    granted: 10,
                },
                OwnerCardinalityV1 {
                    owner: "ops".into(),
                    stored_keys_observed: 37,
                    truncated: false,
                    skipped: false,
                    query_ms: 12,
                    budget_state: "request".into(),
                    source: "rocksdb-exact".into(),
                    estimate: 41,
                    granted: 100,
                },
                OwnerCardinalityV1 {
                    owner: "zzz".into(),
                    stored_keys_observed: 0,
                    truncated: false,
                    skipped: true,
                    query_ms: 0,
                    budget_state: "unmeasured".into(),
                    source: "zenoh-walk".into(),
                    estimate: 0,
                    granted: 0,
                },
            ],
            stored_keys_observed_total: 137,
            owners_truncated: 1,
            owners_skipped: 1,
            admin_declarations: 847,
            admin_census_ms: 19,
            scoped_get_ms: 2,
            crawl_ms: 1234,
            lazy_threshold: 100,
            declare_rate_proxy_per_s: 0.5,
            proxy_interval_s: 60,
            budget_source_state: "live".into(),
            zenoh_session_state: "live".into(),
            zenoh_session_detail: "connected".into(),
        };
        let j = band_json(&r, 8);
        // The fields the router-health band + Prometheus bridge already read.
        assert_eq!(j["stored_keys_total"], 137);
        assert_eq!(j["admin_declarations"], 847);
        assert_eq!(j["admin_walk_ms"], 19);
        assert_eq!(j["scoped_get_ms"], 2);
        assert_eq!(j["router"], "zagros");
        // Top owners sorted desc, skipped owners excluded.
        let top = j["top_owners_by_stored_keys"].as_array().unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0]["owner"], "a2dc1592");
        assert_eq!(top[0]["truncated"], true);
        // New crawler fields present.
        assert_eq!(j["lazy_threshold"], 100);
        assert_eq!(j["owners_skipped"], 1);
        assert_eq!(j["source"], "goat-fabric-crawler");
        // Layer 2b: allocation surface — full non-skipped owner set with
        // granted alongside, plus the source-health field.
        assert_eq!(j["budget_source_state"], "live");
        assert_eq!(top[0]["granted"], 10);
        let ob = j["owners_budget"].as_array().unwrap();
        assert_eq!(
            ob.len(),
            2,
            "skipped owners excluded from the budget surface"
        );
        assert_eq!(ob[0]["owner"], "a2dc1592");
        assert_eq!(ob[0]["granted"], 10);
        assert_eq!(ob[0]["budget_state"], "exception");
        assert_eq!(ob[1]["owner"], "ops");
        assert_eq!(ob[1]["granted"], 100);
    }

    #[test]
    fn band_json_legacy_mode_has_no_budget_surface() {
        let r = FabricCardinalityV1 {
            site: "nova".into(),
            sampled_at_ms: 1,
            owners: vec![OwnerCardinalityV1 {
                owner: "x".into(),
                stored_keys_observed: 5,
                truncated: false,
                skipped: false,
                query_ms: 3,
                budget_state: "self".into(),
                source: "zenoh-walk".into(),
                estimate: 0,
                granted: 0,
            }],
            stored_keys_observed_total: 5,
            owners_truncated: 0,
            owners_skipped: 0,
            admin_declarations: 1,
            admin_census_ms: 1,
            scoped_get_ms: 1,
            crawl_ms: 1,
            lazy_threshold: 100,
            declare_rate_proxy_per_s: 0.0,
            proxy_interval_s: 0,
            budget_source_state: String::new(), // decoded-from-old-publisher shape
            zenoh_session_state: String::new(), // ditto — an old publisher always had one
            zenoh_session_detail: String::new(),
        };
        let j = band_json(&r, 8);
        // An empty (old-publisher) state renders as the explicit "none".
        assert_eq!(j["budget_source_state"], "none");
        assert!(
            j["owners_budget"].is_null(),
            "no allocation source => no budget surface"
        );
        // An old publisher always held a session, so its facets are real.
        assert_eq!(j["zenoh_session_state"], "live");
        assert_eq!(j["admin_declarations"], 1);
    }

    #[test]
    fn repeated_secondary_opens_do_not_accumulate_log_files() {
        // The crawler opens one secondary per store per cycle. RocksDB renames
        // LOG -> LOG.old.<ts> on every open, so without keep_log_file_num the
        // count grows once per cycle forever, to millions of files and hundreds
        // of GiB of pure rotation debris across a large store set.
        //
        // Asserts the BEHAVIOUR (file count stays bounded across many opens),
        // not that a setter was called. That distinction earned its keep: the
        // first attempt at this fix was `set_keep_log_file_num(2)`, which reads
        // exactly like a fix and left 24 files across these 25 opens, because
        // RocksDB does not run its info-LOG purge for secondary instances. A
        // test asserting the option had been set would have passed and shipped
        // a no-op.
        let base = std::env::temp_dir().join(format!("fc-logrot-{}", std::process::id()));
        let primary = base.join("primary");
        let secondary = base.join("secondary");
        let _ = std::fs::remove_dir_all(&base);
        {
            let mut opts = rocksdb::Options::default();
            opts.create_if_missing(true);
            opts.create_missing_column_families(true);
            let db =
                rocksdb::DB::open_cf(&opts, &primary, ["default", "data_info"]).expect("create");
            let cf = db.cf_handle("default").unwrap();
            db.put_cf(&cf, b"ops/a", b"x").unwrap();
            db.flush().unwrap();
        }
        std::fs::create_dir_all(&secondary).expect("mkdir secondary");

        // 25 cycles: comfortably past keep_log_file_num, and past the point
        // where an unbounded implementation is unambiguously distinguishable.
        for _ in 0..25 {
            rocksdb_store_census(&primary, &secondary, 1_000_000).expect("census");
        }

        let log_olds = std::fs::read_dir(&secondary)
            .expect("read secondary")
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("LOG.old.")
            })
            .count();
        let _ = std::fs::remove_dir_all(&base);

        assert!(
            log_olds <= 4,
            "secondary dir accumulated {log_olds} LOG.old files across 25 opens; \
             keep_log_file_num is not bounding rotation (this is the 188.9 GiB fault)"
        );
    }

    #[test]
    fn rocksdb_census_counts_one_cf_not_all() {
        // The backend keeps 2 CFs (payload + data_info) with one entry per
        // key each — counting all CFs doubles (rig-verified, M-091). This
        // guards the single-CF rule against a live-shaped store.
        let base = std::env::temp_dir().join(format!("fc-test-{}", std::process::id()));
        let primary = base.join("primary");
        let secondary = base.join("secondary");
        let _ = std::fs::remove_dir_all(&base);
        {
            let mut opts = rocksdb::Options::default();
            opts.create_if_missing(true);
            opts.create_missing_column_families(true);
            let db =
                rocksdb::DB::open_cf(&opts, &primary, ["default", "data_info"]).expect("create");
            let payload_cf = db.cf_handle("default").unwrap();
            let info_cf = db.cf_handle("data_info").unwrap();
            for i in 0..137 {
                let k = format!("video/frame/{i}");
                db.put_cf(&payload_cf, k.as_bytes(), b"x").unwrap();
                db.put_cf(&info_cf, k.as_bytes(), b"meta").unwrap();
            }
            db.flush().unwrap();
        }
        let (exact, estimate, capped) =
            rocksdb_store_census(&primary, &secondary, 1_000_000).expect("census");
        assert_eq!(exact, 137, "one CF counted, not both");
        assert!(!capped);
        assert!(estimate > 0, "estimate property readable");
        // Cap honesty: iteration stops at the cap and reports capped.
        let (exact2, _, capped2) =
            rocksdb_store_census(&primary, &base.join("secondary2"), 50).expect("census capped");
        assert_eq!(exact2, 50);
        assert!(capped2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rollup_roundtrips_protobuf() {
        let r = FabricCardinalityV1 {
            site: "zagros".into(),
            sampled_at_ms: 1,
            owners: vec![OwnerCardinalityV1 {
                owner: "x".into(),
                stored_keys_observed: 5,
                truncated: false,
                skipped: false,
                query_ms: 3,
                budget_state: "self".into(),
                source: "rocksdb-exact".into(),
                estimate: 5,
                granted: 10,
            }],
            stored_keys_observed_total: 5,
            owners_truncated: 0,
            owners_skipped: 0,
            admin_declarations: 10,
            admin_census_ms: 4,
            scoped_get_ms: 1,
            crawl_ms: 9,
            lazy_threshold: 100,
            declare_rate_proxy_per_s: 0.0,
            proxy_interval_s: 0,
            budget_source_state: "live".into(),
            zenoh_session_state: "live".into(),
            zenoh_session_detail: "connected".into(),
        };
        let bytes = r.encode_to_vec();
        let back = FabricCardinalityV1::decode(bytes.as_slice()).expect("decode");
        assert_eq!(back.site, "zagros");
        assert_eq!(back.owners.len(), 1);
        assert_eq!(back.owners[0].budget_state, "self");
        assert_eq!(back.owners[0].granted, 10);
        assert_eq!(back.budget_source_state, "live");
    }

    #[test]
    fn budget_state_granted_is_grant_relative_and_honest() {
        // No grant on record => unmeasured, NEVER a violation.
        assert_eq!(
            budget_state_granted(5000, false, 10, 100, None),
            "unmeasured"
        );
        assert_eq!(
            budget_state_granted(5000, false, 10, 100, Some(0)),
            "unmeasured"
        );
        // Skipped stays unmeasured regardless of the grant.
        assert_eq!(
            budget_state_granted(0, true, 10, 100, Some(10)),
            "unmeasured"
        );
        // Within grant: the label names the APPROVAL TIER of the allowance.
        assert_eq!(budget_state_granted(7, false, 10, 100, Some(10)), "self");
        assert_eq!(
            budget_state_granted(70, false, 10, 100, Some(100)),
            "request"
        );
        assert_eq!(
            budget_state_granted(10_700, false, 10, 100, Some(20_000)),
            "granted"
        );
        // Over grant => exception, at every allowance size.
        assert_eq!(
            budget_state_granted(11, false, 10, 100, Some(10)),
            "exception"
        );
        assert_eq!(
            budget_state_granted(101, false, 10, 100, Some(100)),
            "exception"
        );
        assert_eq!(
            budget_state_granted(20_001, false, 10, 100, Some(20_000)),
            "exception"
        );
        // Boundary: observed == granted is within.
        assert_eq!(budget_state_granted(10, false, 10, 100, Some(10)), "self");
    }

    #[test]
    fn budget_map_parses_and_resolves() {
        let raw = br#"{
            "generated_at": "2026-07-31T00:00:00Z",
            "self_service_block": 10,
            "auto_self_service_anchors": ["aaa", "bbb"],
            "grants": {
                "ops": {"granted": 20000, "note": "infra"},
                "bbb": {"granted": 100, "note": "sponsor-approved"}
            }
        }"#;
        let m = BudgetMap::parse(raw).expect("parse");
        // Automatic self-service block for an onboarded slot.
        assert_eq!(m.granted_for("aaa"), Some(10));
        // Explicit grant overrides the automatic block.
        assert_eq!(m.granted_for("bbb"), Some(100));
        assert_eq!(m.granted_for("ops"), Some(20000));
        // Unknown owner: no grant on record.
        assert_eq!(m.granted_for("zzz"), None);
        // classify_owner wires state + wire-granted together.
        let (state, granted) = classify_owner("ops", 10_700, false, false, 10, 100, Some(&m));
        assert_eq!((state, granted), ("granted", 20000));
        let (state, granted) = classify_owner("zzz", 470, false, false, 10, 100, Some(&m));
        assert_eq!((state, granted), ("unmeasured", 0));
        // Legacy mode (no budget source): global-tier awareness, granted=0.
        let (state, granted) = classify_owner("zzz", 470, false, false, 10, 100, None);
        assert_eq!((state, granted), ("exception", 0));
        // A malformed grants row fails the parse (fail-closed => stale/unavailable
        // handling upstream, never a silently-partial map).
        assert!(BudgetMap::parse(br#"{"self_service_block": 10, "grants": {"x": {}}}"#).is_err());
        assert!(BudgetMap::parse(b"not json").is_err());
    }

    fn args_for(extra: &[&str]) -> Args {
        let mut argv = vec![
            "goat-fabric-crawler",
            "--endpoint",
            "tls/router:7447",
            "--site",
            "zagros",
        ];
        argv.extend_from_slice(extra);
        Args::parse_from(argv)
    }

    #[test]
    fn a_zenoh_session_is_required_only_without_a_usable_rocksdb_source() {
        let dir = std::env::temp_dir().join(format!("fc-mode-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let d = dir.to_string_lossy().to_string();

        // rocksdb-direct mode active: the census is router-independent, so a
        // session failure degrades facets instead of killing the sensor.
        let a = args_for(&["--rocksdb-dir", &d]);
        assert!(rocksdb_base(&a).is_some());
        assert!(!session_required(rocksdb_base(&a).as_deref()));

        // Configured but unreadable is NOT the mode — the router is the only
        // census source left, so the session stays required (fail fast).
        let missing = dir.join("nope");
        let a = args_for(&["--rocksdb-dir", missing.to_str().unwrap()]);
        assert!(rocksdb_base(&a).is_none());
        assert!(session_required(rocksdb_base(&a).as_deref()));

        // Unconfigured: same — zenoh-walk mode cannot run without a session.
        let a = args_for(&[]);
        assert!(session_required(rocksdb_base(&a).as_deref()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rocksdb_census_and_budget_classification_survive_a_dead_session() {
        // Item 9 of the fragility audit: with no zenoh session the
        // rocksdb-direct census + budget classification still run (they never
        // touch the router) and the router-dependent facets are LABELED
        // unavailable rather than fabricated as zero or dropped.
        let base = std::env::temp_dir().join(format!("fc-degraded-{}", std::process::id()));
        let stores = base.join("stores");
        let secondary = base.join("secondary");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(stores.join("a2dc1592")).expect("mkdir store");
        // The secondary BASE is the caller's to create (main does it once per
        // run); rocksdb creates only its own per-store dir under it.
        std::fs::create_dir_all(&secondary).expect("mkdir secondary");
        {
            let mut opts = rocksdb::Options::default();
            opts.create_if_missing(true);
            opts.create_missing_column_families(true);
            let db = rocksdb::DB::open_cf(&opts, stores.join("a2dc1592"), ["default", "data_info"])
                .expect("create");
            let payload_cf = db.cf_handle("default").unwrap();
            for i in 0..12 {
                db.put_cf(&payload_cf, format!("video/frame/{i}").as_bytes(), b"x")
                    .unwrap();
            }
            db.flush().unwrap();
        }
        let budget = BudgetMap::parse(
            br#"{"self_service_block": 10, "auto_self_service_anchors": ["a2dc1592"]}"#,
        )
        .expect("parse budget");

        let rows = rocksdb_census(&stores, &secondary, 1_000_000, 100, 1_000, Some(&budget));
        assert_eq!(rows.len(), 1, "the census ran with no session at all");
        assert_eq!(rows[0].source, "rocksdb-exact", "store read exactly");
        assert!(!rows[0].skipped);
        assert_eq!(rows[0].stored_keys_observed, 12);
        // Grant-relative classification is router-independent too: 12 observed
        // against a 10-key automatic block is an exception, session or not.
        assert_eq!(rows[0].budget_state, "exception");
        assert_eq!(rows[0].granted, 10);

        let r = FabricCardinalityV1 {
            site: "zagros".into(),
            sampled_at_ms: 1_700_000_000_000,
            stored_keys_observed_total: rows.iter().map(|o| o.stored_keys_observed).sum(),
            owners_truncated: 0,
            owners_skipped: 0,
            owners: rows,
            // Placeholders: not measured this cycle (no session to measure with).
            admin_declarations: 0,
            admin_census_ms: 0,
            scoped_get_ms: 0,
            crawl_ms: 7,
            lazy_threshold: 100,
            declare_rate_proxy_per_s: 0.0,
            proxy_interval_s: 0,
            budget_source_state: "live".into(),
            zenoh_session_state: "unavailable".into(),
            zenoh_session_detail: "no-transport".into(),
        };
        let j = band_json(&r, 8);
        // The router-independent half is real and present.
        assert_eq!(j["stored_keys_total"], 12);
        assert_eq!(j["top_owners_by_stored_keys"][0]["owner"], "a2dc1592");
        assert_eq!(j["owners_budget"][0]["granted"], 10);
        assert_eq!(j["budget_source_state"], "live");
        // The router-dependent half is labeled unavailable and reads null —
        // never a confident zero.
        assert_eq!(j["zenoh_session_state"], "unavailable");
        assert!(j["admin_declarations"].is_null());
        assert!(j["admin_walk_ms"].is_null());
        assert!(j["scoped_get_ms"].is_null());
        assert!(j["note"].as_str().unwrap().contains("DEGRADED"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn no_census_source_reports_every_owner_unmeasured() {
        // Session down AND the rocksdb mount gone: an empty census would read
        // as "the fabric is empty". Every remembered owner is unmeasured.
        let budget = BudgetMap::parse(
            br#"{"self_service_block": 10, "auto_self_service_anchors": ["aaa"]}"#,
        )
        .expect("parse budget");
        let row = unmeasured_row("aaa", "zenoh-unavailable", 100, 1_000, Some(&budget));
        assert!(row.skipped);
        assert_eq!(row.stored_keys_observed, 0);
        assert_eq!(row.budget_state, "unmeasured");
        assert_eq!(row.source, "zenoh-unavailable");
        // The grant on record still rides along — the allowance is known even
        // though the usage is not.
        assert_eq!(row.granted, 10);
    }

    #[test]
    fn heartbeat_path_resolves_identically_for_daemon_and_probe() {
        // Explicit wins.
        assert_eq!(
            heartbeat_path(Some("/hb/beat.json"), Some("/out/zenoh-fabric.json")),
            std::path::PathBuf::from("/hb/beat.json")
        );
        // Else a sibling of the band JSON — that dir is an existing writable
        // bind mount wherever json_out is configured.
        assert_eq!(
            heartbeat_path(None, Some("/out/zenoh-fabric.json")),
            std::path::PathBuf::from("/out/fabric-crawler-heartbeat.json")
        );
        // Empty strings are not configuration.
        assert_eq!(
            heartbeat_path(Some(""), Some("")),
            std::path::PathBuf::from("/tmp/fabric-crawler-heartbeat.json")
        );
        assert_eq!(
            heartbeat_path(None, None),
            std::path::PathBuf::from("/tmp/fabric-crawler-heartbeat.json")
        );
        // Pointing the heartbeat AT the band JSON is refused — the two writers
        // would collide on the same .tmp name and the heartbeat would silently
        // overwrite the surface the router-health band reads.
        assert_eq!(
            heartbeat_path(
                Some("/out/zenoh-fabric.json"),
                Some("/out/zenoh-fabric.json")
            ),
            std::path::PathBuf::from("/out/fabric-crawler-heartbeat.json")
        );
    }

    #[test]
    fn healthcheck_is_liveness_and_never_reports_router_state() {
        let now = 1_700_000_000_000_i64;
        // A crawler crawling from rocksdb with a DEAD session is HEALTHY. This
        // is the whole point of pairing the probe with item 9: if a router
        // outage turned the container unhealthy, docker would re-couple the
        // crawler to the router the census does not need.
        let degraded = heartbeat_doc(
            now - 30_000,
            240_000,
            "cycle-complete",
            17,
            120,
            Some(&FabricCardinalityV1 {
                site: "zagros".into(),
                sampled_at_ms: now - 30_000,
                owners: vec![],
                stored_keys_observed_total: 0,
                owners_truncated: 0,
                owners_skipped: 0,
                admin_declarations: 0,
                admin_census_ms: 0,
                scoped_get_ms: 0,
                crawl_ms: 120,
                lazy_threshold: 100,
                declare_rate_proxy_per_s: 0.0,
                proxy_interval_s: 0,
                budget_source_state: "live".into(),
                zenoh_session_state: "unavailable".into(),
                zenoh_session_detail: "no-transport".into(),
            }),
        );
        let (healthy, why) = healthcheck_verdict(Some(&degraded), now, "/tmp/hb.json");
        assert!(healthy, "degraded session must not fail the probe: {why}");
        assert!(why.contains("zenoh_session=unavailable"), "reported: {why}");

        // Fresh startup write, before any cycle: healthy.
        let starting = heartbeat_doc(now - 1_000, 240_000, "starting", 0, 0, None);
        assert!(healthcheck_verdict(Some(&starting), now, "/tmp/hb.json").0);

        // Past its stamped window: unhealthy, with the age named.
        let stale = heartbeat_doc(now - 600_000, 240_000, "cycle-complete", 3, 90, None);
        let (healthy, why) = healthcheck_verdict(Some(&stale), now, "/tmp/hb.json");
        assert!(!healthy);
        assert!(why.starts_with("UNHEALTHY: heartbeat 600s old"), "{why}");

        // Missing or wrong-schema heartbeat: unhealthy, and the reason names
        // the path so an operator can see it is a mount problem, not a wedge.
        let (healthy, why) = healthcheck_verdict(None, now, "/tmp/hb.json");
        assert!(!healthy);
        assert!(why.contains("/tmp/hb.json"), "{why}");
        let (healthy, why) =
            healthcheck_verdict(Some(&serde_json::json!({"nope": 1})), now, "/tmp/hb.json");
        assert!(!healthy);
        assert!(why.contains("wrong schema"), "{why}");
    }

    #[test]
    fn heartbeat_round_trips_through_the_file_the_probe_reads() {
        let dir = std::env::temp_dir().join(format!("fc-hb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("fabric-crawler-heartbeat.json");
        let now = now_unix_ms();
        write_heartbeat(
            &path,
            &heartbeat_doc(now, 240_000, "cycle-complete", 5, 88, None),
        );
        let raw = std::fs::read(&path).expect("heartbeat written");
        let doc: serde_json::Value = serde_json::from_slice(&raw).expect("valid json");
        assert_eq!(doc["schema"], "fabric-crawler-heartbeat/v1");
        assert_eq!(doc["cycles_completed"], 5);
        assert_eq!(doc["healthy_until_ms"].as_i64().unwrap(), now + 240_000);
        assert!(healthcheck_verdict(Some(&doc), now, "x").0);
        // No .tmp left behind.
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_state_is_transport_truth_not_object_existence() {
        // THE F-280 ROW: the crawler holds a Session, the router is gone.
        // This reported "live" through an outage, which keeps
        // goat_zenoh_crawler_session_live pinned at 1 and leaves
        // ZenohCrawlerSessionDegraded dark for the exact case it was written
        // for.
        assert_eq!(
            session_state(true, false),
            ("unavailable", "no-transport"),
            "a held Session with no transport is NOT live"
        );
        // Never opened at all — same state, different triage answer.
        assert_eq!(session_state(false, false), ("unavailable", "no-session"));
        // Connected is the only way to be live.
        assert_eq!(session_state(true, true), ("live", "connected"));
        // The state vocabulary stays exactly two values, because consumers map
        // `!= "unavailable"` to trustworthy (zenoh-health-to-prom.py). A third
        // state would read as trustworthy while the facets were not.
        for (has, reach) in [(true, true), (true, false), (false, false)] {
            let (state, _) = session_state(has, reach);
            assert!(
                state == "live" || state == "unavailable",
                "unexpected state {state}"
            );
        }
    }

    /// A session with nowhere to connect: the object opens, the transport
    /// table stays empty. This is the shape of the bug — object alive,
    /// nothing reachable — reproduced without needing a router to kill.
    // Multi-thread flavor required: the zenoh runtime refuses tokio's
    // current-thread scheduler, which is what #[tokio::test] gives by default.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn held_session_with_no_transport_is_not_reachable() {
        let mut cfg = zenoh::Config::default();
        cfg.insert_json5("mode", "\"peer\"").unwrap();
        cfg.insert_json5("scouting/multicast/enabled", "false")
            .unwrap();
        cfg.insert_json5("listen/endpoints", "[]").unwrap();
        cfg.insert_json5("connect/endpoints", "[]").unwrap();
        let session = zenoh::open(cfg).await.expect("session opens");
        // The object exists — this is precisely what the old code called
        // "live".
        assert!(!session.is_closed());
        assert!(
            !router_reachable(&session).await,
            "no transport must not read as reachable"
        );
        let _ = session.close().await;
    }

    /// Transport comes up, then goes away underneath a session that is never
    /// dropped — and the predicate follows it in both directions.
    ///
    /// Two peers over TCP stand in for the client→router link this crawler
    /// actually uses: `router_reachable` checks routers AND peers, and zenoh
    /// resolves both from the same runtime transport table, filtered only by
    /// whatami (zenoh 1.9.0 `runtime::get_zids`). The client→router direction
    /// itself is validated live, not here — spinning a real router in
    /// unit tests is not worth the dependency.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn transport_loss_and_recovery_flip_reachability() {
        // Free port, then release it for zenoh to bind.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            l.local_addr().expect("addr").port()
        };
        let endpoint = format!("tcp/127.0.0.1:{port}");

        let mut listen_cfg = zenoh::Config::default();
        listen_cfg.insert_json5("mode", "\"peer\"").unwrap();
        listen_cfg
            .insert_json5("scouting/multicast/enabled", "false")
            .unwrap();
        listen_cfg
            .insert_json5("listen/endpoints", &format!("[\"{endpoint}\"]"))
            .unwrap();
        let listener = zenoh::open(listen_cfg).await.expect("listener opens");

        let mut client_cfg = zenoh::Config::default();
        client_cfg.insert_json5("mode", "\"peer\"").unwrap();
        client_cfg
            .insert_json5("scouting/multicast/enabled", "false")
            .unwrap();
        client_cfg.insert_json5("listen/endpoints", "[]").unwrap();
        client_cfg
            .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
            .unwrap();
        let client = zenoh::open(client_cfg).await.expect("client opens");

        // Up: the transport establishes shortly after open.
        assert!(
            wait_for_reachable(&client, true).await,
            "connected peer should read reachable"
        );

        // Down: kill the far end WITHOUT touching the client session, which
        // is exactly what a router restart does to a running crawler.
        let _ = listener.close().await;
        assert!(
            wait_for_reachable(&client, false).await,
            "transport loss must flip reachability to false while the Session object is still held"
        );
        // The object is emphatically still alive — the old signal would still
        // have said "live" right here.
        assert!(!client.is_closed());

        let _ = client.close().await;
    }

    /// Poll `router_reachable` until it matches `want`, up to ~10s. Transport
    /// establishment and teardown are asynchronous; a bare assert would be
    /// flaky in both directions.
    async fn wait_for_reachable(session: &zenoh::Session, want: bool) -> bool {
        for _ in 0..100 {
            if router_reachable(session).await == want {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }
}
