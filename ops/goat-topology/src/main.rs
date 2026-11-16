// SPDX-License-Identifier: Apache-2.0
//! goat-topology — the zenoh routing-topology surface (ADR 1160).
//!
//! ```text
//!   goat-topology publish   on each router host: read MY router, publish it
//!   goat-topology serve     on the mgmt-host:    subscribe, render, grade
//! ```
//!
//! The split is not stylistic. The router adminspace binds to its host's
//! loopback, so only a co-located process can read it; and a surface that
//! queried each router per page view would be exactly the shape ADR 1123
//! forbids. Self-publish + passive roll-up is also what survives a partition:
//! a cut-off site keeps publishing and the surface shows its last snapshot with
//! an honest age (ADR 1036).

mod adminspace;
mod assets;
mod declarations;
mod directory;
mod findings;
mod gwconfig;
mod keys;
mod serve;
mod store;
mod view;
mod wire;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use wire::{GatewayMode, RouterTopologyV1};

#[derive(Parser, Debug)]
#[command(
    name = "goat-topology",
    about = "Zenoh routing-topology surface (ADR 1160)."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run on a zenoh router host: read this router, publish what it says.
    Publish(PublishArgs),
    /// Run on the mgmt-host: subscribe, hold in memory, serve the surface.
    Serve(ServeArgs),
    /// Print what this host's router looks like, as JSON, and exit. Publishes
    /// nothing and opens no zenoh session — so it is safe to run against a
    /// production router, and it is how an operator answers "what does
    /// topology think my config says?" without waiting for a publish cycle.
    Inspect(InspectArgs),
}

#[derive(Parser, Debug)]
struct InspectArgs {
    #[arg(long, env = "TOPOLOGY_ADMIN_ADDR", default_value = "127.0.0.1:8000")]
    admin_addr: String,
    #[arg(long, env = "TOPOLOGY_CONFIG_PATH")]
    config_path: Option<String>,
    /// Read the declared config only; do not touch the adminspace. Lets the
    /// config parser be exercised off-host, against a captured file.
    #[arg(long, default_value_t = false)]
    config_only: bool,

    /// When to read the route table: `settled` (default) reads it only when
    /// the routing map holds still for `--route-table-settle-ms`; `always`
    /// reads it every cycle; `off` never does. A route-table read that lands
    /// just after a router joins or leaves can crash a zenoh 1.9.0 router
    /// (zenoh#2831); see README.md, "Known issues".
    #[arg(long, env = "TOPOLOGY_ROUTE_TABLE", value_enum, default_value_t = adminspace::RouteTable::Settled)]
    route_table: adminspace::RouteTable,
    /// How long the routing map must hold still before `settled` reads the
    /// route table.
    #[arg(long, env = "TOPOLOGY_ROUTE_TABLE_SETTLE_MS", default_value_t = 1000)]
    route_table_settle_ms: u64,
}

#[derive(Parser, Debug)]
struct PublishArgs {
    /// Zenoh router endpoint(s) to PUBLISH through, e.g.
    /// `tls/goat-zenoh-router:7447`. Comma-separated; tried IN ORDER, and the
    /// first that admits this client is used. Name this host's own router
    /// first and a neighbor router after it: a router whose `gateway.south` is
    /// a Custom list with no subregion for clients refuses every default
    /// client, this one included, and the neighbor is how its snapshot still
    /// reaches the bus. What is READ is always `--admin-addr`, never the
    /// router published through.
    #[arg(
        long,
        env = "TOPOLOGY_ENDPOINT",
        value_delimiter = ',',
        required = true
    )]
    endpoint: Vec<String>,
    /// `<site>` chunk of the key `<prefix>/<site>/<node>`.
    #[arg(long, env = "TOPOLOGY_SITE")]
    site: String,
    /// `<node>` chunk — this router. Must be unique within the site.
    #[arg(long, env = "TOPOLOGY_NODE")]
    node: String,
    /// Key prefix: snapshots are published on `<prefix>/<site>/<node>`, and
    /// `serve` subscribes to `<prefix>/**`. Publishers and the surface must
    /// use the same value. With access control on, the publisher needs `put`
    /// and the surface `declare_subscriber` on `<prefix>/**`.
    #[arg(long, env = "TOPOLOGY_KEY_PREFIX", default_value = keys::DEFAULT_PREFIX)]
    key_prefix: String,

    /// The local router's adminspace REST address. Loopback by default: the
    /// router publishes 8000/tcp to 127.0.0.1, so no netns entry is needed.
    #[arg(long, env = "TOPOLOGY_ADMIN_ADDR", default_value = "127.0.0.1:8000")]
    admin_addr: String,
    /// The router's DECLARED config file. Only its `gateway` block is read;
    /// the ACL that dominates this file is never parsed, logged or published.
    #[arg(long, env = "TOPOLOGY_CONFIG_PATH")]
    config_path: Option<String>,

    #[arg(long, env = "TOPOLOGY_INTERVAL_SECS", default_value_t = 30)]
    interval_secs: u64,

    /// When to read the route table: `settled` (default) reads it only when
    /// the routing map holds still for `--route-table-settle-ms`; `always`
    /// reads it every cycle; `off` never does. A route-table read that lands
    /// just after a router joins or leaves can crash a zenoh 1.9.0 router
    /// (zenoh#2831); see README.md, "Known issues".
    #[arg(long, env = "TOPOLOGY_ROUTE_TABLE", value_enum, default_value_t = adminspace::RouteTable::Settled)]
    route_table: adminspace::RouteTable,
    /// How long the routing map must hold still before `settled` reads the
    /// route table.
    #[arg(long, env = "TOPOLOGY_ROUTE_TABLE_SETTLE_MS", default_value_t = 1000)]
    route_table_settle_ms: u64,

    /// How often to read the subscriber and publisher tables for the
    /// declarations census. 0 turns the census off. Slower than the snapshot
    /// on purpose: these are the large tables, and what they count changes as
    /// applications start and stop, not second to second.
    #[arg(
        long,
        env = "TOPOLOGY_DECLARATIONS_INTERVAL_SECS",
        default_value_t = 300
    )]
    declarations_interval_secs: u64,
    /// Time budget for the two census reads together. A read that runs out
    /// publishes "not read", never zeros.
    #[arg(long, env = "TOPOLOGY_DECLARATIONS_TIMEOUT_SECS", default_value_t = 10)]
    declarations_timeout_secs: u64,
    /// Entries counted per table; past it the census is marked capped.
    #[arg(
        long,
        env = "TOPOLOGY_DECLARATIONS_MAX_ENTRIES",
        default_value_t = 20000
    )]
    declarations_max_entries: usize,
    /// Key expressions sampled per router (0 = counts only).
    #[arg(long, env = "TOPOLOGY_DECLARATIONS_TOP_KEYS", default_value_t = 5)]
    declarations_top_keys: usize,

    /// Client certificate (PEM) for mutual TLS. With `--key`; leave both out
    /// to authenticate only the router.
    #[arg(long, env = "TOPOLOGY_CERT")]
    cert: Option<String>,
    #[arg(long, env = "TOPOLOGY_KEY_FILE")]
    key: Option<String>,
    /// CA certificate (PEM) the router's certificate is verified against.
    /// Required for a tls/ or quic/ endpoint.
    #[arg(long, env = "TOPOLOGY_ROOT_CA")]
    root_ca: Option<String>,
    /// With a tls/ or quic/ endpoint, also check the router's certificate
    /// against the host name in the endpoint. Off by default.
    #[arg(long, env = "TOPOLOGY_TLS_VERIFY_NAME", default_value_t = false)]
    tls_verify_name: bool,
}

#[derive(Parser, Debug)]
struct ServeArgs {
    #[arg(long, env = "TOPOLOGY_ENDPOINT")]
    endpoint: String,
    /// Restrict to one site's routers. Omit to roll up every site the
    /// subscription can see.
    #[arg(long, env = "TOPOLOGY_SITE")]
    site: Option<String>,
    /// Key prefix: snapshots are published on `<prefix>/<site>/<node>`, and
    /// `serve` subscribes to `<prefix>/**`. Publishers and the surface must
    /// use the same value. With access control on, the publisher needs `put`
    /// and the surface `declare_subscriber` on `<prefix>/**`.
    #[arg(long, env = "TOPOLOGY_KEY_PREFIX", default_value = keys::DEFAULT_PREFIX)]
    key_prefix: String,
    #[arg(long, env = "TOPOLOGY_BIND", default_value = "0.0.0.0:8080")]
    bind: String,

    /// A file on this host that maps a mesh address to a peer name, hostname
    /// and groups: the relay-health peer-names file or the mesh-health peer
    /// registry. Read when it changes; never fetched from the mesh controller.
    /// Omit and routers that send no report keep their derived names.
    #[arg(long, env = "TOPOLOGY_PEER_DIRECTORY")]
    peer_directory: Option<std::path::PathBuf>,
    /// The identity registry's public-safe roster (`roster.json`): the
    /// organisation, organisation nation and individual for a peer whose name
    /// leads to a roster handle.
    #[arg(long, env = "TOPOLOGY_ROSTER")]
    roster: Option<std::path::PathBuf>,
    /// This goatnet's name, as it appears in person-device peer names
    /// (`peer-<handle>-<goatnet>-<date>-<host>`), so it is not read as part of
    /// the handle.
    #[arg(long, env = "TOPOLOGY_GOATNET", default_value = "")]
    goatnet: String,
    /// Leave the individual (and their nationality) out of the payload. The
    /// organisation and its nation are still shown.
    #[arg(long, env = "TOPOLOGY_HIDE_INDIVIDUAL", default_value_t = false)]
    hide_individual: bool,

    #[arg(long, env = "TOPOLOGY_CERT")]
    cert: Option<String>,
    #[arg(long, env = "TOPOLOGY_KEY_FILE")]
    key: Option<String>,
    /// CA certificate (PEM) the router's certificate is verified against.
    /// Required for a tls/ or quic/ endpoint.
    #[arg(long, env = "TOPOLOGY_ROOT_CA")]
    root_ca: Option<String>,
    /// With a tls/ or quic/ endpoint, also check the router's certificate
    /// against the host name in the endpoint. Off by default.
    #[arg(long, env = "TOPOLOGY_TLS_VERIFY_NAME", default_value_t = false)]
    tls_verify_name: bool,
}

/// Shared zenoh client config. Mirrors goat-fabric-health so both meters fail
/// the same way against the same router.
struct Tls<'a> {
    endpoint: &'a str,
    cert: Option<&'a String>,
    key: Option<&'a String>,
    root_ca: Option<&'a String>,
    verify_name: bool,
}

/// The `transport/link/tls` block for one endpoint, or `None` when the
/// endpoint is not TLS.
///
/// - `tls/` and `quic/` endpoints both use this block (QUIC carries TLS).
/// - The router's certificate is always verified against `--root-ca`.
/// - With `--cert` and `--key`, the client presents a certificate (mutual
///   TLS); with neither, it does not. One without the other is refused, since
///   it is never what was meant.
/// - `--tls-verify-name` also checks the router's certificate against the
///   host name in the endpoint. Off by default, which is what every
///   deployment before the setting used (they connect by address).
///
/// Built as JSON, not by formatting strings, so a path with a quote or a
/// backslash in it cannot break the config.
fn link_tls(t: &Tls<'_>) -> Result<Option<serde_json::Value>> {
    if !(t.endpoint.starts_with("tls/") || t.endpoint.starts_with("quic/")) {
        return Ok(None);
    }
    let root_ca = t.root_ca.context(
        "--root-ca is required for a tls/ or quic/ endpoint: the router's certificate is \
         verified against it",
    )?;
    let mut block = serde_json::json!({
        "root_ca_certificate": root_ca,
        "verify_name_on_connect": t.verify_name,
    });
    match (t.cert, t.key) {
        (Some(cert), Some(key)) => {
            block["connect_certificate"] = cert.as_str().into();
            block["connect_private_key"] = key.as_str().into();
            block["enable_mtls"] = true.into();
        }
        (None, None) => block["enable_mtls"] = false.into(),
        _ => anyhow::bail!(
            "--cert and --key go together: give both for mutual TLS, or neither to \
             authenticate only the router"
        ),
    }
    Ok(Some(block))
}

async fn open_session(t: Tls<'_>) -> Result<zenoh::Session> {
    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("config mode: {e}"))?;
    config
        .insert_json5(
            "connect/endpoints",
            &serde_json::to_string(&[t.endpoint]).expect("a string list serializes"),
        )
        .map_err(|e| anyhow::anyhow!("config connect/endpoints: {e}"))?;
    if let Some(block) = link_tls(&t)? {
        config
            .insert_json5("transport/link/tls", &block.to_string())
            .map_err(|e| anyhow::anyhow!("config transport/link/tls: {e}"))?;
    }
    zenoh::open(config)
        .await
        .map_err(|e| anyhow::anyhow!("open zenoh session: {e}"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build one snapshot of this router.
///
/// Never returns `Err` for a read failure: a snapshot that says "I could not
/// read my own adminspace" is more useful than silence, because silence is
/// indistinguishable from a dead publisher and would be graded `unknown`
/// without saying why.
async fn snapshot(a: &PublishArgs) -> RouterTopologyV1 {
    let mut snap = RouterTopologyV1 {
        site: a.site.clone(),
        node: a.node.clone(),
        ts_unix_ms: now_ms(),
        interval_secs: a.interval_secs as u32,
        ..Default::default()
    };

    // --- declared: the gateway block of the config file --------------------
    match a.config_path.as_deref() {
        Some(p) => {
            let d = gwconfig::read(p);
            snap.gateway_mode = d.mode as i32;
            snap.custom_subregions = d.subregions;
            snap.client_admission = d.admission as i32;
            snap.gateway_read_error = d.error;
        }
        None => {
            snap.gateway_mode = GatewayMode::Unreadable as i32;
            snap.gateway_read_error =
                "no --config-path given, so the declared gateway posture is unknown; \
                 it is NOT assumed to be auto"
                    .into();
        }
    }

    // --- observed: the adminspace ------------------------------------------
    if a.admin_addr.is_empty() {
        snap.observe_error = "adminspace not read (--config-only)".into();
        return snap;
    }
    let policy = adminspace::RoutePolicy {
        mode: a.route_table,
        settle_ms: a.route_table_settle_ms,
    };
    match adminspace::fetch_snapshot(&a.admin_addr, 10, policy).await {
        Err(e) => snap.observe_error = format!("{e:#}"),
        Ok(read) => match adminspace::parse(&read.body) {
            Err(e) => snap.observe_error = format!("{e:#}"),
            Ok(obs) => {
                adminspace::apply(&mut snap, obs);
                snap.route_table_note = read.route_table_note;
            }
        },
    }
    snap
}

/// Read this router's subscriber and publisher tables once and count them.
///
/// Never returns `Err`: a census that could not be read is published as
/// `read_error` with no counts, so the page says "not read" rather than zero.
async fn census(a: &PublishArgs) -> wire::DeclarationsV1 {
    let started = std::time::Instant::now();
    let read = async {
        let t = a.declarations_timeout_secs;
        let subs = adminspace::get_body(&a.admin_addr, "@/local/router/subscriber/**", t).await?;
        let pubs = adminspace::get_body(&a.admin_addr, "@/local/router/publisher/**", t).await?;
        Ok::<_, anyhow::Error>((subs, pubs))
    };
    let budget = std::time::Duration::from_secs(a.declarations_timeout_secs);
    let result = match tokio::time::timeout(budget, read).await {
        Err(_) => Err(anyhow::anyhow!(
            "census read timed out after {}s",
            a.declarations_timeout_secs
        )),
        Ok(r) => r.and_then(|(subs, pubs)| {
            declarations::census(
                &subs,
                &pubs,
                declarations::Limits {
                    max_entries: a.declarations_max_entries,
                    top_keys: a.declarations_top_keys,
                },
            )
        }),
    };
    let mut d = result.unwrap_or_else(|e| wire::DeclarationsV1 {
        read_error: format!("not read: {e:#}"),
        ..Default::default()
    });
    d.read_unix_ms = now_ms();
    d.interval_secs = a.declarations_interval_secs as u32;
    d.read_ms = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
    d
}

/// Say what a refusal means, so the log line is the diagnosis.
fn interpret_open_error(e: &str) -> &'static str {
    if e.contains("conflicts with auto preset") {
        "this router's gateway.south is a Custom list with no subregion that matches clients, \
         so it puts every default client north of itself and the client refuses (finding \
         T-013); trying the next endpoint"
    } else {
        "trying the next endpoint"
    }
}

/// Open a client session on the first endpoint that admits it.
///
/// One endpoint per attempt, not all of them in one `connect/endpoints` list:
/// the order is the operator's statement of preference (own router first), and
/// a per-endpoint attempt is what lets the log say WHICH router refused and why.
async fn open_first(a: &PublishArgs) -> Result<(zenoh::Session, String)> {
    let mut why = Vec::new();
    for ep in a
        .endpoint
        .iter()
        .map(|e| e.trim())
        .filter(|e| !e.is_empty())
    {
        match open_session(Tls {
            endpoint: ep,
            cert: a.cert.as_ref(),
            key: a.key.as_ref(),
            root_ca: a.root_ca.as_ref(),
            verify_name: a.tls_verify_name,
        })
        .await
        {
            Ok(s) => return Ok((s, ep.to_string())),
            Err(e) => {
                let e = format!("{e:#}");
                tracing::warn!(endpoint = %ep, err = %e, "{}", interpret_open_error(&e));
                why.push(format!("{ep}: {e}"));
            }
        }
    }
    anyhow::bail!("no endpoint admitted this publisher — {}", why.join("; "))
}

async fn run_publish(a: PublishArgs) -> Result<()> {
    // The publisher reads its own router's declared config anyway, so it can
    // say BEFORE connecting that the router will refuse it — zenoh's own error
    // for this is a bare "Unable to connect".
    if let Some(p) = a.config_path.as_deref() {
        if gwconfig::read(p).admission == wire::ClientAdmission::Refused {
            let n = a.endpoint.len();
            tracing::warn!(
                endpoints = n,
                "this router's gateway.south has no subregion that matches clients, so it will \
                 refuse this publisher as it refuses every default client (finding T-013). {}",
                if n > 1 {
                    "The endpoints after the first are tried in order."
                } else {
                    "Give --endpoint a neighbor router after this one, or add a client subregion \
                     to the gateway block."
                }
            );
        }
    }
    keys::check_prefix(&a.key_prefix)?;
    keys::check_chunk("--site", &a.site)?;
    keys::check_chunk("--node", &a.node)?;
    let (session, via) = open_first(&a).await?;

    let key = keys::router_key(&a.key_prefix, &a.site, &a.node);
    tracing::info!(key = %key, admin = %a.admin_addr, via = %via, "publishing router topology");

    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(a.interval_secs));
    // The census runs on the first tick and then every
    // `--declarations-interval-secs`; between reads every snapshot carries the
    // last one, with the time it was read.
    let mut decl: Option<wire::DeclarationsV1> = None;
    let mut next_census = tokio::time::Instant::now();
    // Shape notes are logged when they change, not every cycle.
    let mut last_notes: Vec<String> = Vec::new();
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let mut snap = snapshot(&a).await;
                if a.declarations_interval_secs > 0 && tokio::time::Instant::now() >= next_census {
                    let d = census(&a).await;
                    if d.read_error.is_empty() {
                        tracing::info!(read_ms = d.read_ms, topics = d.topics, routers = d.routers.len(), capped = d.capped, "declarations census read");
                    } else {
                        tracing::warn!(err = %d.read_error, read_ms = d.read_ms, "declarations census not read");
                    }
                    decl = Some(d);
                    next_census = tokio::time::Instant::now()
                        + std::time::Duration::from_secs(a.declarations_interval_secs);
                }
                snap.declarations = decl.clone();
                if !snap.observe_error.is_empty() {
                    tracing::warn!(err = %snap.observe_error, "adminspace read failed; publishing the failure");
                }
                if snap.shape_notes != last_notes {
                    for n in &snap.shape_notes {
                        tracing::warn!(note = %n, "adminspace shape not as expected; publishing what was read");
                    }
                    last_notes = snap.shape_notes.clone();
                }
                if let Err(e) = session.put(&key, snap.encode_vec()).await {
                    tracing::warn!("publish {key}: {e}");
                }
            }
            _ = shutdown_signal() => {
                tracing::info!("shutting down");
                return Ok(());
            }
        }
    }
}

/// Resolves on Ctrl-C (SIGINT) or, on Unix, SIGTERM.
///
/// SIGTERM is what `docker stop`, `docker compose down`, systemd and
/// Kubernetes send. Waiting only for SIGINT left every stop to run out its
/// grace period (10 s by default in docker) and end in SIGKILL.
pub(crate) async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(e) => {
                tracing::warn!("cannot listen for SIGTERM ({e}); only Ctrl-C stops this process");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match Cli::parse().cmd {
        Cmd::Publish(a) => run_publish(a).await,
        Cmd::Inspect(a) => {
            let snap = snapshot(&PublishArgs {
                endpoint: vec![],
                site: "-".into(),
                node: "-".into(),
                key_prefix: keys::DEFAULT_PREFIX.into(),
                admin_addr: if a.config_only {
                    String::new()
                } else {
                    a.admin_addr
                },
                config_path: a.config_path,
                interval_secs: 0,
                route_table: a.route_table,
                route_table_settle_ms: a.route_table_settle_ms,
                declarations_interval_secs: 0,
                declarations_timeout_secs: 10,
                declarations_max_entries: 20000,
                declarations_top_keys: 5,
                cert: None,
                key: None,
                root_ca: None,
                tls_verify_name: false,
            })
            .await;
            println!("{}", serde_json::to_string_pretty(&snap)?);
            Ok(())
        }
        Cmd::Serve(a) => {
            // The surface reads only what publishers put on the bus. It never
            // queries a router, so a page view can never become fabric load
            // (ADR 1123) — the routers it draws but cannot hear from are drawn
            // from OTHER routers' link-state, already in their snapshots.
            keys::check_prefix(&a.key_prefix)?;
            if let Some(s) = &a.site {
                keys::check_chunk("--site", s)?;
            }
            let session = open_session(Tls {
                endpoint: &a.endpoint,
                cert: a.cert.as_ref(),
                key: a.key.as_ref(),
                root_ca: a.root_ca.as_ref(),
                verify_name: a.tls_verify_name,
            })
            .await?;
            let sources =
                directory::Sources::new(a.peer_directory, a.roster, a.goatnet, !a.hide_individual);
            serve::run(session, &a.key_prefix, a.site, &a.bind, sources).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t<'a>(
        endpoint: &'a str,
        cert: Option<&'a String>,
        key: Option<&'a String>,
        root_ca: Option<&'a String>,
    ) -> Tls<'a> {
        Tls {
            endpoint,
            cert,
            key,
            root_ca,
            verify_name: false,
        }
    }

    #[test]
    fn plain_tcp_has_no_tls_block() {
        assert!(link_tls(&t("tcp/192.0.2.1:7447", None, None, None))
            .unwrap()
            .is_none());
    }

    #[test]
    fn mutual_tls_is_what_every_deployment_had() {
        let (c, k, ca) = (
            "c.pem".to_string(),
            "k.pem".to_string(),
            "ca.pem".to_string(),
        );
        let b = link_tls(&t("tls/192.0.2.1:7447", Some(&c), Some(&k), Some(&ca)))
            .unwrap()
            .unwrap();
        assert_eq!(b["enable_mtls"], true);
        assert_eq!(b["connect_certificate"], "c.pem");
        assert_eq!(b["connect_private_key"], "k.pem");
        assert_eq!(b["root_ca_certificate"], "ca.pem");
        assert_eq!(b["verify_name_on_connect"], false);
    }

    #[test]
    fn server_only_tls_and_quic_need_only_the_ca() {
        let ca = "ca.pem".to_string();
        for ep in [
            "tls/router.example.org:7447",
            "quic/router.example.org:7447",
        ] {
            let b = link_tls(&t(ep, None, None, Some(&ca))).unwrap().unwrap();
            assert_eq!(b["enable_mtls"], false, "{ep}");
            assert!(b.get("connect_certificate").is_none(), "{ep}");
        }
    }

    #[test]
    fn half_a_client_identity_or_no_ca_is_refused() {
        let (c, ca) = ("c.pem".to_string(), "ca.pem".to_string());
        let e = link_tls(&t("tls/x:7447", Some(&c), None, Some(&ca)))
            .unwrap_err()
            .to_string();
        assert!(e.contains("go together"), "{e}");
        let e = link_tls(&t("tls/x:7447", None, None, None))
            .unwrap_err()
            .to_string();
        assert!(e.contains("--root-ca"), "{e}");
    }

    #[test]
    fn a_path_with_a_quote_cannot_break_the_config() {
        let ca = r#"/etc/zenoh/ca "a\b".pem"#.to_string();
        let b = link_tls(&t("tls/x:7447", None, None, Some(&ca)))
            .unwrap()
            .unwrap();
        let mut config = zenoh::Config::default();
        config
            .insert_json5("transport/link/tls", &b.to_string())
            .expect("the block parses whatever the path holds");
        assert_eq!(b["root_ca_certificate"], ca.as_str());
    }
}
