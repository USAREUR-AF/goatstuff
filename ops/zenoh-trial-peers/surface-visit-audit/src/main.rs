// SPDX-License-Identifier: Apache-2.0
//
// goat-surface-visit — per-visit attribution for oauth2-proxy-fenced surfaces.
//
// WHAT PROBLEM THIS SOLVES. Hydra records one consent grant per (person,
// client). That answers "has this person reached brief, and when last" and
// nothing more: repeat visits collapse into one row with a moving timestamp.
// This binary supplies the per-visit half — counts, and a history with paths
// and times — by reading the only place that holds both the validated identity
// and the request line, which is the door's own request log.
//
// WHY IT READS A LOG RATHER THAN THE APP. Two of the three surfaces this was
// built for are static file servers. There is no application to instrument, and
// adding one would mean shipping code to serve a deck. The door already
// authenticates every request and already writes the identity down; this reads
// what is written instead of asking anything to be rewritten.
//
// WHAT IT REFUSES TO RECORD.
//   * Requests with no authenticated user (oauth2-proxy renders "-"). That is
//     every prober, every scrape, and every api_route 401 — traffic that must
//     never appear as a person visiting a page.
//   * Query strings. A query can carry the content of a search, which is a
//     different and more sensitive thing than the fact of a visit.
//   * Anything, in plaintext, onto the bus. Every record is age-encrypted to
//     the operator roster before it is published, so a mis-set read ACL leaks
//     opaque ciphertext rather than a browsing trail. This binary holds only
//     public recipients and cannot read back what it writes.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use prost::Message;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

// ---------------------------------------------------------------------------
// SurfaceVisitV1 — hand-written prost struct mirroring
// proto/surface_visit_v1.proto field-for-field, tag-for-tag. Same no-codegen
// convention as fabric-health and operator-action-audit. A change in the .proto
// that is not mirrored here breaks the wire silently, which is why both files
// say so at the top.
// ---------------------------------------------------------------------------
#[derive(Clone, PartialEq, Message)]
pub struct SurfaceVisitV1 {
    #[prost(string, tag = "1")]
    pub actor: String,
    #[prost(string, tag = "2")]
    pub surface: String,
    #[prost(string, tag = "3")]
    pub env: String,
    #[prost(string, tag = "4")]
    pub path: String,
    #[prost(string, tag = "5")]
    pub method: String,
    #[prost(uint32, tag = "6")]
    pub status: u32,
    #[prost(uint32, tag = "7")]
    pub duration_ms: u32,
    #[prost(string, tag = "8")]
    pub host: String,
    #[prost(int64, tag = "9")]
    pub ts_unix_ms: i64,
    #[prost(string, tag = "10")]
    pub node: String,
}

/// One line of the door's request log, in the JSON shape the
/// oauth2-proxy-surface role pins via OAUTH2_PROXY_REQUEST_LOGGING_FORMAT.
/// Every field is optional on purpose: a format drift upstream should drop the
/// field, not kill the tailer, and the missing-field case is counted and
/// reported rather than silently skipped.
#[derive(Debug, Deserialize)]
struct DoorLogLine {
    ts: Option<String>,
    user: Option<String>,
    host: Option<String>,
    method: Option<String>,
    path: Option<String>,
    status: Option<u32>,
    dur: Option<f64>,
}

#[derive(Parser, Debug)]
#[command(name = "goat-surface-visit", version, about)]
struct Args {
    /// Surface this door fronts. Equal to its Hydra client_id, so a visit row
    /// joins to the consent grant microscope already reads.
    #[arg(long, env = "SURFACE_VISIT_SURFACE")]
    surface: String,

    /// goatnet label, e.g. zagros.
    #[arg(long, env = "SURFACE_VISIT_ENV")]
    env: String,

    /// This node's id. Two nodes serving one surface produce two streams and a
    /// roll-up must not silently merge them.
    #[arg(long, env = "SURFACE_VISIT_NODE")]
    node: String,

    /// The door's request log as a plain file. Mutually exclusive with
    /// --door-container; one of the two is required.
    #[arg(long, env = "SURFACE_VISIT_LOG_PATH")]
    log_path: Option<PathBuf>,

    /// Resolve the log by DOCKER CONTAINER NAME instead of a fixed path.
    ///
    /// This exists because the json-file log lives at
    /// <root>/containers/<id>/<id>-json.log and the id changes every time the
    /// container is recreated — which this project's own role does on any
    /// config change. A path pinned at deploy time therefore goes stale
    /// silently: the tailer keeps reading a file nothing writes any more,
    /// reports zero visits, and looks healthy. That is the F-177 shape.
    ///
    /// Resolution reads <root>/containers/*/config.v2.json for the container's
    /// "Name" field. It needs read access to the containers directory and
    /// deliberately NOT the docker socket, which would be a far larger grant
    /// for a log tailer.
    #[arg(long, env = "SURFACE_VISIT_DOOR_CONTAINER")]
    door_container: Option<String>,

    /// Docker root's containers directory, mounted read-only. On hardened hosts
    /// the docker root is /opt/docker-data/docker, not the /var/lib/docker
    /// default, so this is not defaulted to
    /// the upstream path — a wrong default here resolves nothing and reads as
    /// an idle surface.
    #[arg(long, env = "SURFACE_VISIT_DOCKER_CONTAINERS_DIR")]
    docker_containers_dir: Option<PathBuf>,

    /// age recipients (X25519 public keys), one per line — the operator roster.
    /// PUBLIC keys only: this binary encrypts and can never decrypt.
    #[arg(long, env = "SURFACE_VISIT_AGE_RECIPIENTS_FILE")]
    age_recipients_file: PathBuf,

    /// Zenoh endpoint, e.g. tls/zenoh-router:7447.
    #[arg(long, env = "SURFACE_VISIT_ZENOH_ENDPOINT")]
    endpoint: String,

    #[arg(long, env = "SURFACE_VISIT_ZENOH_CERT")]
    cert: Option<String>,
    #[arg(long, env = "SURFACE_VISIT_ZENOH_KEY")]
    key: Option<String>,
    #[arg(long, env = "SURFACE_VISIT_ZENOH_ROOT_CA")]
    root_ca: Option<String>,

    /// Start at the end of the log rather than replaying it. Default true: on
    /// a restart the door's whole retained log would otherwise be re-emitted
    /// as if those visits had just happened.
    #[arg(long, env = "SURFACE_VISIT_TAIL_ONLY", default_value_t = true)]
    tail_only: bool,

    /// Emit nothing; log what WOULD be published. For a first look at a new
    /// surface without writing to the audit stream.
    #[arg(long, env = "SURFACE_VISIT_DRY_RUN", default_value_t = false)]
    dry_run: bool,
}

/// Find a container's json-file log by NAME, without the docker socket.
/// Returns None when the name matches nothing, so the caller can retry rather
/// than treat a mid-recreate gap as an empty surface.
fn resolve_container_log(containers_dir: &PathBuf, name: &str) -> Option<PathBuf> {
    let want = format!("\"Name\":\"/{name}\"");
    // Report a permission failure AS one. Docker's containers directory is
    // root:root 0710, so an unprivileged reader gets EACCES here — and
    // swallowing that with .ok() turned "I am not allowed to look" into "no
    // container named X", which sent the first deploy of this component hunting
    // a container that was running the whole time.
    let entries = match std::fs::read_dir(containers_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            tracing::error!(
                dir = %containers_dir.display(),
                "cannot read the container directory (permission denied). Docker's \
                 containers dir is root:root 0710; an unprivileged process cannot \
                 list it. Prefer --log-path against a log the door writes itself."
            );
            return None;
        }
        Err(e) => {
            tracing::error!(dir = %containers_dir.display(), error = %e, "cannot read the container directory");
            return None;
        }
    };
    for e in entries.flatten() {
        let dir = e.path();
        let cfg = dir.join("config.v2.json");
        let Ok(body) = std::fs::read_to_string(&cfg) else { continue };
        if !body.contains(&want) {
            continue;
        }
        let id = dir.file_name()?.to_string_lossy().to_string();
        let log = dir.join(format!("{id}-json.log"));
        if log.exists() {
            return Some(log);
        }
    }
    None
}

/// Docker's json-file driver wraps each line: {"log":"...","stream":"stdout",...}.
/// Unwrap it when present so the same binary works against a bind-mounted
/// container log and against a plain file.
fn unwrap_docker(line: &str) -> String {
    #[derive(Deserialize)]
    struct Envelope {
        log: String,
    }
    match serde_json::from_str::<Envelope>(line) {
        Ok(e) => e.log.trim_end().to_string(),
        Err(_) => line.to_string(),
    }
}

/// oauth2-proxy renders an absent username as "-". Anything without a real one
/// is a prober, a scrape, or an api_route 401 — never a person visiting a page.
fn authenticated_user(raw: &Option<String>) -> Option<String> {
    match raw {
        Some(u) if !u.is_empty() && u != "-" => Some(u.clone()),
        _ => None,
    }
}

/// A query string can carry the content of a search. This records that a page
/// was visited, not what was typed into it.
fn strip_query(path: &str) -> String {
    match path.split_once('?') {
        Some((p, _)) => p.to_string(),
        None => path.to_string(),
    }
}

/// The door stamps `2026/09/05 15:01:25`. Parse it as UTC; on failure return
/// None so the caller can count the line as unparseable rather than dating it
/// "now" — substituting read time for event time is the lie microscope's
/// never-re-stamp rule exists to prevent.
fn parse_ts_ms(raw: &Option<String>) -> Option<i64> {
    let s = raw.as_ref()?;
    chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y/%m/%d %H:%M:%S")
        .ok()
        .map(|dt| dt.and_utc().timestamp_millis())
}

fn load_recipients(path: &PathBuf) -> Result<Vec<age::x25519::Recipient>> {
    use std::str::FromStr;
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("read age recipients {}", path.display()))?;
    let mut out: Vec<age::x25519::Recipient> = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let r = age::x25519::Recipient::from_str(line)
            .map_err(|e| anyhow::anyhow!("bad age recipient {line:?}: {e}"))?;
        out.push(r);
    }
    anyhow::ensure!(
        !out.is_empty(),
        "no age recipients in {} — refusing to run: an empty roster would publish \
         plaintext visit history, which is the one thing this must never do",
        path.display()
    );
    Ok(out)
}

/// Encrypt to the roster, binary (non-armored) age. Same shape as goat-cli's
/// encrypt_to_roster (crates/goat-cli/src/cmd/audit.rs) so one envelope format
/// covers both audit streams and a single reader can open either.
fn age_encrypt(plaintext: &[u8], recipients: &[age::x25519::Recipient]) -> Result<Vec<u8>> {
    use std::io::Write;
    let recips = recipients.iter().map(|r| r as &dyn age::Recipient);
    let encryptor = age::Encryptor::with_recipients(recips)
        .map_err(|e| anyhow::anyhow!("age encryptor: {e}"))?;
    let mut out = Vec::new();
    let mut writer = encryptor
        .wrap_output(&mut out)
        .map_err(|e| anyhow::anyhow!("age wrap_output: {e}"))?;
    writer.write_all(plaintext)?;
    writer.finish().map_err(|e| anyhow::anyhow!("age finish: {e}"))?;
    Ok(out)
}

async fn open_session(args: &Args) -> Result<zenoh::Session> {
    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("config mode: {e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{}\"]", args.endpoint))
        .map_err(|e| anyhow::anyhow!("config connect/endpoints: {e}"))?;
    if args.endpoint.starts_with("tls/") {
        let cert = args.cert.as_ref().context("--cert required for a tls/ endpoint")?;
        let key = args.key.as_ref().context("--key required for a tls/ endpoint")?;
        let root_ca = args
            .root_ca
            .as_ref()
            .context("--root-ca required for a tls/ endpoint")?;
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

#[derive(Default)]
struct Counters {
    lines: u64,
    unparseable: u64,
    anonymous: u64,
    /// Samples HANDED TO THE LOCAL SESSION — not deliveries. A Zenoh put is
    /// fire-and-forget: Ok means the session accepted the sample, never that
    /// the router did. This counter read 1 for two hours while every publish
    /// was being denied by an ACL that had not loaded. It is
    /// named for what it measures so nobody reads it as delivery again.
    handed_to_session: u64,
    no_timestamp: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "goat_surface_visit=info".into()),
        )
        .init();

    let args = Args::parse();
    let recipients = load_recipients(&args.age_recipients_file)?;
    tracing::info!(
        surface = %args.surface,
        env = %args.env,
        node = %args.node,
        recipients = recipients.len(),
        dry_run = args.dry_run,
        "starting; visits will be age-encrypted to the operator roster"
    );

    let session = if args.dry_run {
        None
    } else {
        Some(open_session(&args).await.context("open zenoh session")?)
    };

    // Resolve the log once at start. Either a fixed path was given, or the
    // container name is resolved to its current json-file log.
    let log_path: PathBuf = match (&args.log_path, &args.door_container, &args.docker_containers_dir) {
        (Some(p), _, _) => p.clone(),
        (None, Some(name), Some(dir)) => {
            let mut found = None;
            // A container recreated at the moment this starts is normal, so
            // retry briefly rather than exiting and letting the restart loop
            // decide.
            for _ in 0..30 {
                if let Some(p) = resolve_container_log(dir, name) {
                    found = Some(p);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            found.with_context(|| {
                format!(
                    "no container named {name:?} with a json-file log under {} after 60s — \
                     check the docker root: this fleet uses /opt/docker-data/docker, not \
                     the /var/lib/docker default",
                    dir.display()
                )
            })?
        }
        _ => anyhow::bail!(
            "give either --log-path, or both --door-container and --docker-containers-dir"
        ),
    };
    tracing::info!(path = %log_path.display(), "tailing the door log");

    let file = tokio::fs::File::open(&log_path)
        .await
        .with_context(|| format!("open door log {}", log_path.display()))?;
    let mut reader = BufReader::new(file);

    if args.tail_only {
        // Seek to the end so a restart does not replay retained history as if
        // it had just happened.
        use tokio::io::AsyncSeekExt;
        reader.get_mut().seek(std::io::SeekFrom::End(0)).await?;
    }

    let mut counters = Counters::default();
    let mut line = String::new();
    let mut report = tokio::time::interval(std::time::Duration::from_secs(300));
    report.tick().await;

    loop {
        line.clear();
        tokio::select! {
            read = reader.read_line(&mut line) => {
                let n = read?;
                if n == 0 {
                    // EOF: the door is idle. Wait rather than spin.
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
                counters.lines += 1;
                let raw = unwrap_docker(line.trim_end());
                if raw.is_empty() || !raw.trim_start().starts_with('{') {
                    // oauth2-proxy's standard + auth logs share this stream and
                    // are not JSON. Not an error; not a visit either.
                    continue;
                }
                let parsed: DoorLogLine = match serde_json::from_str(&raw) {
                    Ok(p) => p,
                    Err(_) => { counters.unparseable += 1; continue; }
                };
                let Some(actor) = authenticated_user(&parsed.user) else {
                    counters.anonymous += 1;
                    continue;
                };
                let Some(ts_unix_ms) = parse_ts_ms(&parsed.ts) else {
                    counters.no_timestamp += 1;
                    continue;
                };

                let visit = SurfaceVisitV1 {
                    actor: actor.clone(),
                    surface: args.surface.clone(),
                    env: args.env.clone(),
                    path: strip_query(parsed.path.as_deref().unwrap_or("")),
                    method: parsed.method.unwrap_or_default(),
                    status: parsed.status.unwrap_or(0),
                    duration_ms: parsed.dur.map(|d| (d * 1000.0) as u32).unwrap_or(0),
                    host: parsed.host.unwrap_or_default(),
                    ts_unix_ms,
                    node: args.node.clone(),
                };

                let encoded = visit.encode_to_vec();
                let sealed = age_encrypt(&encoded, &recipients)?;

                // The actor is a key segment, so a read ACL can be scoped per
                // person. It is an email; '/' would split the key, so it is
                // replaced rather than allowed to reshape the namespace.
                let actor_seg = actor.replace('/', "_");
                let key = format!(
                    "ops/audit/surface/{}/{}",
                    args.surface, actor_seg
                );

                if let Some(s) = &session {
                    if let Err(e) = s.put(&key, sealed).await {
                        tracing::warn!(error = %e, key = %key, "publish failed");
                        continue;
                    }
                } else {
                    tracing::info!(key = %key, actor = %actor, path = %visit.path,
                        status = visit.status, "DRY RUN — would publish");
                }
                counters.handed_to_session += 1;
            }
            _ = report.tick() => {
                // Report the shape of what was skipped, not just what was sent.
                // "0 visits" is ambiguous between an idle surface and a broken
                // parse, and these counters are what tell them apart.
                tracing::info!(
                    lines = counters.lines,
                    handed_to_session = counters.handed_to_session,
                    anonymous = counters.anonymous,
                    unparseable = counters.unparseable,
                    no_timestamp = counters.no_timestamp,
                    // handed_to_session is NOT a delivery count — see F-464.
                    // Read the key back off the router to know a sample landed.
                    "five-minute roll-up (handed_to_session counts local puts, not deliveries)"
                );
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(handed_to_session = counters.handed_to_session, "shutting down");
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_requests_are_not_visits() {
        // oauth2-proxy renders an absent user as "-". Probers, scrapes and
        // api_route 401s all arrive this way and must never be counted.
        assert!(authenticated_user(&Some("-".into())).is_none());
        assert!(authenticated_user(&Some("".into())).is_none());
        assert!(authenticated_user(&None).is_none());
        assert_eq!(
            authenticated_user(&Some("someone@example.invalid".into())),
            Some("someone@example.invalid".to_string())
        );
    }

    #[test]
    fn query_strings_are_dropped() {
        assert_eq!(strip_query("/search?q=secret+thing"), "/search");
        assert_eq!(strip_query("/plain"), "/plain");
        assert_eq!(strip_query("/trailing?"), "/trailing");
    }

    #[test]
    fn door_timestamps_parse_and_bad_ones_do_not_become_now() {
        let ok = parse_ts_ms(&Some("2026/09/05 15:01:25".into()));
        assert!(ok.is_some());
        // An unparseable stamp must yield None so the line is counted as
        // undated, never silently stamped with read time.
        assert!(parse_ts_ms(&Some("not a date".into())).is_none());
        assert!(parse_ts_ms(&None).is_none());
    }

    #[test]
    fn docker_json_envelope_is_unwrapped_and_plain_lines_pass_through() {
        let wrapped = r#"{"log":"{\"user\":\"a@b.c\"}\n","stream":"stdout"}"#;
        assert_eq!(unwrap_docker(wrapped), r#"{"user":"a@b.c"}"#);
        assert_eq!(unwrap_docker("plain line"), "plain line");
    }

    #[test]
    fn a_real_door_line_becomes_a_visit() {
        let raw = r#"{"ts":"2026/09/05 15:01:25","user":"someone@example.invalid","host":"brief.zagros.example.invalid","method":"GET","path":"/deck?page=3","status":200,"dur":0.004,"client":"172.18.0.1"}"#;
        let p: DoorLogLine = serde_json::from_str(raw).expect("parses");
        assert_eq!(authenticated_user(&p.user).unwrap(), "someone@example.invalid");
        assert_eq!(strip_query(p.path.as_deref().unwrap()), "/deck");
        assert_eq!(p.status.unwrap(), 200);
        assert_eq!((p.dur.unwrap() * 1000.0) as u32, 4);
    }

    #[test]
    fn a_container_log_is_resolved_by_name_not_by_a_pinned_id() {
        // The id in the log path changes on every container recreate, which
        // this project's own role does on any config change. Resolution by
        // name is what stops a stale path reading as an idle surface.
        let root = std::env::temp_dir().join("goat-surface-visit-test-resolve");
        let _ = std::fs::remove_dir_all(&root);
        let id = "abc123def456";
        let cdir = root.join(id);
        std::fs::create_dir_all(&cdir).unwrap();
        std::fs::write(cdir.join("config.v2.json"), r#"{"Name":"/oauth2-proxy-brief","x":1}"#).unwrap();
        std::fs::write(cdir.join(format!("{id}-json.log")), "").unwrap();

        assert_eq!(
            resolve_container_log(&root, "oauth2-proxy-brief"),
            Some(cdir.join(format!("{id}-json.log")))
        );
        // A near-miss name must not match: oauth2-proxy-brief-old is a
        // different container.
        assert_eq!(resolve_container_log(&root, "oauth2-proxy-brie"), None);
        assert_eq!(resolve_container_log(&root, "oauth2-proxy-dataflows"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_roster_is_refused() {
        // Publishing visit history in plaintext because a roster file happened
        // to be empty is the one failure this must not have.
        let dir = std::env::temp_dir().join("goat-surface-visit-test-empty-roster");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("recipients.txt");
        std::fs::write(&p, "# only a comment\n").unwrap();
        assert!(load_recipients(&p).is_err());
        std::fs::remove_file(&p).ok();
    }
}

#[cfg(test)]
mod tls_tests;
