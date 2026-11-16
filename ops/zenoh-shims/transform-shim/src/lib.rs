//! transform-shim: a fabric peer that converts messages between formats.
//!
//! The fabric is brokerless, so there is no central place to transform a
//! message. A transformer is a peer: it subscribes to keys that carry one
//! format, converts each message with the goat-transformers library, and
//! republishes it under another key. Which keys, which formats and where the
//! result goes is a route table read from one file at start.
//!
//! Rules this crate holds (docs/design/format-transformers.md):
//!
//! * One message in gives at most one message out. The shim never queries
//!   the fabric and answers no query.
//! * The subscriber callback never waits. It hands the sample to a bounded
//!   queue; when the queue is full the sample is dropped and counted.
//! * A route table that could feed itself is refused at start.
//! * Every refusal is counted per route, logged, and published in the
//!   heartbeat. Nothing fails silently.
//! * The packs come from a bundle whose every file is checked against its
//!   manifest, and the engine loads nothing marked `controlled` unless told to.
//!
//! License: Apache-2.0

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use goat_transformers::{Distribution, Engine, Pack, Wire};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{error, info, warn};
use zenoh::bytes::Encoding;
use zenoh::key_expr::KeyExpr;

/// The class name in `services/shims/<class>/…` and `health/shims/<class>/…`.
pub const SHIM_CLASS: &str = "transform";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// The route table file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Names this instance in its announcement and heartbeat keys.
    pub instance: String,
    /// The site this instance runs at; carried in the announcement.
    pub site: String,
    /// Put in front of the announcement and heartbeat keys. Empty on a fabric
    /// that grants the reserved top-level prefixes; `<anchor>/` on a fabric
    /// where a peer writes only under its own anchor.
    #[serde(default)]
    pub key_prefix: String,
    pub routes: Vec<RouteConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    pub name: String,
    /// Key expression to subscribe to. It begins with `strip_prefix`.
    pub subscribe: String,
    /// Format of the messages on `subscribe` (a pack name).
    pub from: String,
    /// Format to publish (a pack name).
    pub to: String,
    /// Literal start of every key the route receives.
    pub strip_prefix: String,
    /// Literal start of every key the route publishes. The published key is
    /// `publish_prefix` followed by the received key with `strip_prefix`
    /// removed.
    pub publish_prefix: String,
    /// Zenoh encoding of the published message. Default: by the wire of `to`.
    #[serde(default)]
    pub encoding: Option<String>,
    /// Model keys the caller supplies to every conversion of this route
    /// (the library's context), values as text. A value is fixed text, or a
    /// template that takes values from the arrival of the message: see
    /// [`Template`].
    #[serde(default)]
    pub set: BTreeMap<String, String>,
}

impl Config {
    pub fn from_json(text: &str) -> Result<Config> {
        serde_json::from_str(text).context("the route table is not valid")
    }
}

// ---------------------------------------------------------------------------
// The pack bundle
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Manifest {
    library: ManifestLibrary,
    files: Vec<ManifestFile>,
}
#[derive(Debug, Deserialize)]
struct ManifestLibrary {
    rev: String,
}
#[derive(Debug, Deserialize)]
struct ManifestFile {
    path: String,
    sha256: String,
}

/// The engine with a bundle loaded, and the wire of each format.
pub struct Formats {
    pub engine: Engine,
    pub wires: BTreeMap<String, Wire>,
    /// The library commit the bundle was built from.
    pub library_rev: String,
}

/// Load `<bundle>/model` and `<bundle>/packs` after checking every file
/// against `<bundle>/MANIFEST.json`. A file the manifest does not list, a
/// file whose content differs, or a pack above `max` stops the load.
pub fn load_bundle(bundle: &Path, max: Distribution) -> Result<Formats> {
    let manifest_path = bundle.join("MANIFEST.json");
    let manifest: Manifest = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {}", manifest_path.display()))?,
    )
    .with_context(|| format!("{} is not a bundle manifest", manifest_path.display()))?;

    let mut listed = BTreeMap::new();
    for f in &manifest.files {
        let bytes = std::fs::read(bundle.join(&f.path))
            .with_context(|| format!("bundle file {} is missing", f.path))?;
        if hex::encode(Sha256::digest(&bytes)) != f.sha256 {
            bail!("bundle file {} does not match the manifest", f.path);
        }
        listed.insert(f.path.clone(), bytes);
    }
    for dir in ["model", "packs"] {
        for found in walk(&bundle.join(dir))? {
            let rel = found
                .strip_prefix(bundle)
                .map_err(|e| anyhow!("{e}"))?
                .to_string_lossy()
                .replace('\\', "/");
            if !listed.contains_key(&rel) {
                bail!("bundle file {rel} is not in the manifest");
            }
        }
    }

    let mut engine = Engine::new();
    engine.set_max_distribution(max);
    // The directory loader orders modules by what each one uses.
    engine
        .load_model_dir(&bundle.join("model"))
        .map_err(|e| anyhow!("loading model modules: {e}"))?;
    let mut wires = BTreeMap::new();
    for (path, bytes) in &listed {
        if !path.starts_with("packs/") {
            continue;
        }
        let text = std::str::from_utf8(bytes).with_context(|| format!("{path} is not UTF-8"))?;
        let pack = Pack::parse(text).map_err(|e| anyhow!("{path}: {e}"))?;
        wires.insert(pack.format.clone(), pack.wire);
        engine.load(pack).map_err(|e| anyhow!("{path}: {e}"))?;
    }
    Ok(Formats {
        engine,
        wires,
        library_rev: manifest.library.rev,
    })
}

fn walk(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            out.extend(walk(&path)?);
        } else {
            out.push(path);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// What a route knows about one received message, and `set` templates
// ---------------------------------------------------------------------------

/// The facts of one arrival that a `set` value may use. None of them comes
/// from the payload: they are what only the receiving peer knows.
#[derive(Debug, Clone, Default)]
pub struct Arrival {
    /// The key the message was published on.
    pub key: String,
    /// When this peer received it, epoch milliseconds UTC.
    pub received_ms: i64,
    /// The Zenoh encoding of the sample, as text. Empty when not stated.
    pub encoding: String,
    /// The sample's attachment, when it carries one.
    pub attachment: Option<Vec<u8>>,
    /// The sample's own timestamp (the publisher's or first router's
    /// clock), epoch milliseconds UTC, when it carries one.
    pub timestamp_ms: Option<i64>,
}

impl Arrival {
    /// An arrival on `key`, received now, with no metadata.
    pub fn now(key: &str) -> Arrival {
        Arrival {
            key: key.to_string(),
            received_ms: now_ms(),
            ..Arrival::default()
        }
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Epoch milliseconds as `YYYY-MM-DDTHH:MM:SS.fffZ`.
pub fn iso8601_ms(ms: i64) -> String {
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Days since 1970-01-01 to a civil date (proleptic Gregorian).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

#[derive(Debug, Clone, PartialEq)]
enum Part {
    Text(String),
    ReceivedMs,
    ReceivedAt,
    TimestampMs,
    TimestampAt,
    Key,
    /// Chunk `n` of the key from the start (0, 1, …) or from the end (-1, -2, …).
    KeyChunk(i64),
    /// What wildcard `n` (from 1) of the route's `subscribe` matched.
    Capture(usize),
    Tail,
    Encoding,
    Attachment,
    AttachmentMember(String),
}

/// One `set` value. Text is written as it is; `{name}` is replaced by a fact
/// of the arrival; `{{` and `}}` write a brace.
///
/// | Placeholder | Value |
/// |---|---|
/// | `{received_ms}`, `{received_at}` | when this peer received the message: epoch milliseconds, or `YYYY-MM-DDTHH:MM:SS.fffZ` |
/// | `{timestamp_ms}`, `{timestamp_at}` | the sample's own timestamp, in the same two forms |
/// | `{key}` | the key the message was published on |
/// | `{key.N}` | chunk N of that key: 0 is the first, -1 the last |
/// | `{capture.N}` | what wildcard N of `subscribe` matched, from 1; the chunks a `**` matched are joined with `/` |
/// | `{tail}` | the key with `strip_prefix` removed |
/// | `{encoding}` | the sample's Zenoh encoding |
/// | `{attachment}` | the sample's attachment, as UTF-8 text |
/// | `{attachment.NAME}` | member NAME of an attachment that is a JSON object: a string as it is, a number or boolean as its JSON text |
///
/// A placeholder with no value for a message (no attachment, no timestamp, a
/// key with fewer chunks) leaves that model key out of the context, so the
/// target format's own rules decide: a default applies, or a kind that
/// requires the key refuses the message.
#[derive(Debug, Clone, PartialEq)]
pub struct Template(Vec<Part>);

impl Template {
    pub fn parse(text: &str, wildcards: usize) -> Result<Template> {
        let mut parts = Vec::new();
        let mut lit = String::new();
        let mut rest = text;
        while let Some(at) = rest.find(['{', '}']) {
            lit.push_str(&rest[..at]);
            let tail = &rest[at..];
            if let Some(t) = tail.strip_prefix("{{") {
                lit.push('{');
                rest = t;
                continue;
            }
            if let Some(t) = tail.strip_prefix("}}") {
                lit.push('}');
                rest = t;
                continue;
            }
            if tail.starts_with('}') {
                bail!("`{text}`: a `}}` with no `{{` before it (write `}}}}` for a brace)");
            }
            let end = tail
                .find('}')
                .ok_or_else(|| anyhow!("`{text}`: a `{{` is not closed"))?;
            let name = &tail[1..end];
            let (head, arg) = match name.split_once('.') {
                Some((h, a)) => (h, Some(a)),
                None => (name, None),
            };
            let part = match (head, arg) {
                ("received_ms", None) => Part::ReceivedMs,
                ("received_at", None) => Part::ReceivedAt,
                ("timestamp_ms", None) => Part::TimestampMs,
                ("timestamp_at", None) => Part::TimestampAt,
                ("key", None) => Part::Key,
                ("key", Some(n)) => Part::KeyChunk(
                    n.parse()
                        .map_err(|_| anyhow!("`{{{name}}}`: `{n}` is not a chunk number"))?,
                ),
                ("capture", Some(n)) => match n.parse::<usize>() {
                    Ok(i) if (1..=wildcards).contains(&i) => Part::Capture(i),
                    _ => bail!("`{{{name}}}`: subscribe has {wildcards} wildcard(s)"),
                },
                ("tail", None) => Part::Tail,
                ("encoding", None) => Part::Encoding,
                ("attachment", None) => Part::Attachment,
                ("attachment", Some(m)) if !m.is_empty() => Part::AttachmentMember(m.into()),
                _ => bail!("`{{{name}}}` is not a placeholder"),
            };
            if !lit.is_empty() {
                parts.push(Part::Text(std::mem::take(&mut lit)));
            }
            parts.push(part);
            rest = &tail[end + 1..];
        }
        lit.push_str(rest);
        if !lit.is_empty() || parts.is_empty() {
            parts.push(Part::Text(lit));
        }
        Ok(Template(parts))
    }

    /// True when the value is the same for every message.
    pub fn is_fixed(&self) -> bool {
        self.0.iter().all(|p| matches!(p, Part::Text(_)))
    }

    /// The value for one arrival; `None` when a placeholder has no value.
    pub fn render(&self, a: &Arrival, subscribe: &str, strip_prefix: &str) -> Option<String> {
        let mut out = String::new();
        let mut caps: Option<Vec<String>> = None;
        for part in &self.0 {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::ReceivedMs => out.push_str(&a.received_ms.to_string()),
                Part::ReceivedAt => out.push_str(&iso8601_ms(a.received_ms)),
                Part::TimestampMs => out.push_str(&a.timestamp_ms?.to_string()),
                Part::TimestampAt => out.push_str(&iso8601_ms(a.timestamp_ms?)),
                Part::Key => out.push_str(&a.key),
                Part::KeyChunk(n) => {
                    let chunks: Vec<&str> = a.key.split('/').collect();
                    let i = if *n < 0 { chunks.len() as i64 + n } else { *n };
                    out.push_str(chunks.get(usize::try_from(i).ok()?)?);
                }
                Part::Capture(n) => {
                    if caps.is_none() {
                        caps = Some(captures(subscribe, &a.key)?);
                    }
                    out.push_str(caps.as_ref()?.get(n - 1)?);
                }
                Part::Tail => out.push_str(a.key.strip_prefix(strip_prefix)?),
                Part::Encoding => out.push_str(&a.encoding),
                Part::Attachment => {
                    out.push_str(std::str::from_utf8(a.attachment.as_deref()?).ok()?)
                }
                Part::AttachmentMember(name) => {
                    let v: serde_json::Value =
                        serde_json::from_slice(a.attachment.as_deref()?).ok()?;
                    match v.as_object()?.get(name)? {
                        serde_json::Value::String(t) => out.push_str(t),
                        m @ (serde_json::Value::Number(_) | serde_json::Value::Bool(_)) => {
                            out.push_str(&m.to_string())
                        }
                        _ => return None,
                    }
                }
            }
        }
        Some(out)
    }
}

fn is_wildcard(chunk: &str) -> bool {
    chunk.contains('*')
}

/// Wildcard chunks in a key expression.
pub fn wildcard_count(subscribe: &str) -> usize {
    subscribe.split('/').filter(|c| is_wildcard(c)).count()
}

/// What each wildcard chunk of `subscribe` matched in `key`, in order.
/// `*` and a chunk with `$*` take one chunk; `**` takes none or more, as few
/// as let the rest match. `None` when the key does not have the shape.
pub fn captures(subscribe: &str, key: &str) -> Option<Vec<String>> {
    fn walk(sub: &[&str], key: &[&str], out: &mut Vec<String>) -> bool {
        let Some((first, more)) = sub.split_first() else {
            return key.is_empty();
        };
        if *first == "**" {
            for take in 0..=key.len() {
                out.push(key[..take].join("/"));
                if walk(more, &key[take..], out) {
                    return true;
                }
                out.pop();
            }
            return false;
        }
        let Some((k, rest)) = key.split_first() else {
            return false;
        };
        if is_wildcard(first) {
            out.push((*k).to_string());
            if walk(more, rest, out) {
                return true;
            }
            out.pop();
            return false;
        }
        first == k && walk(more, rest, out)
    }
    let (sub, key): (Vec<&str>, Vec<&str>) =
        (subscribe.split('/').collect(), key.split('/').collect());
    let mut out = Vec::new();
    walk(&sub, &key, &mut out).then_some(out)
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// Counters of one route. Read by the heartbeat.
#[derive(Debug, Default)]
pub struct Counters {
    pub received: AtomicU64,
    pub converted: AtomicU64,
    /// The library refused the message, or the payload was not text where the
    /// format is text.
    pub refused: AtomicU64,
    /// The queue between the subscriber and the converter was full.
    pub dropped: AtomicU64,
    pub publish_failed: AtomicU64,
    pub last_error: Mutex<Option<String>>,
}

pub struct Route {
    pub cfg: RouteConfig,
    from_wire: Wire,
    to_wire: Wire,
    encoding: String,
    /// The `set` values: model key and template, in key order.
    context: Vec<(String, Template)>,
    pub counters: Counters,
}

/// One converted message, ready to publish.
#[derive(Debug, PartialEq)]
pub struct Converted {
    pub key: String,
    pub payload: Vec<u8>,
    pub encoding: String,
}

fn is_binary(wire: Wire) -> bool {
    matches!(wire, Wire::Bits | Wire::Protobuf)
}

fn default_encoding(wire: Wire, format: &str) -> String {
    let mime = match wire {
        Wire::Xml => "application/xml",
        Wire::Json => "application/json",
        Wire::GeoJson => "application/geo+json",
        Wire::Protobuf => "application/protobuf",
        Wire::Bits => "application/octet-stream",
    };
    // Zenoh's `<type>;<schema>` form: the schema names the pack.
    format!("{mime};{format}")
}

fn literal(what: &str, route: &str, value: &str) -> Result<()> {
    if value.is_empty() || !value.ends_with('/') || value.contains('*') || value.contains('$') {
        bail!("route {route}: {what} `{value}` must be a literal key prefix that ends with `/`");
    }
    Ok(())
}

/// The checked route table and the engine it converts with.
pub struct Table {
    pub formats: Formats,
    pub routes: Vec<Route>,
}

impl Table {
    /// Check every route against the loaded formats and against the other
    /// routes. Any finding is an error: the shim does not start on a table
    /// it would have to guess about.
    pub fn build(cfg: &Config, formats: Formats) -> Result<Table> {
        if cfg.routes.is_empty() {
            bail!("the route table has no routes");
        }
        let mut routes = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        for rc in &cfg.routes {
            let name = &rc.name;
            if !names.insert(name.clone()) {
                bail!("route {name}: the name is used twice");
            }
            let wire_of = |format: &str| {
                formats.wires.get(format).copied().ok_or_else(|| {
                    anyhow!("route {name}: format `{format}` is not in the pack bundle")
                })
            };
            let (from_wire, to_wire) = (wire_of(&rc.from)?, wire_of(&rc.to)?);
            literal("strip_prefix", name, &rc.strip_prefix)?;
            literal("publish_prefix", name, &rc.publish_prefix)?;
            let sub = KeyExpr::try_from(rc.subscribe.as_str())
                .map_err(|e| anyhow!("route {name}: subscribe `{}`: {e}", rc.subscribe))?;
            if !rc.subscribe.starts_with(&rc.strip_prefix) {
                bail!(
                    "route {name}: subscribe `{}` does not begin with strip_prefix `{}`",
                    rc.subscribe,
                    rc.strip_prefix
                );
            }
            // No one subscription may match the whole keyspace
            // (goatcore zenoh-data-fabric.md). strip_prefix already forces a
            // literal first chunk; this states the rule where it is checked.
            if sub.as_str().split('/').all(|c| c == "*" || c == "**") {
                bail!(
                    "route {name}: subscribe `{}` names the whole keyspace",
                    rc.subscribe
                );
            }
            let wildcards = wildcard_count(&rc.subscribe);
            let mut context = Vec::new();
            for (key, text) in &rc.set {
                let template = Template::parse(text, wildcards)
                    .map_err(|e| anyhow!("route {name}: set {key}: {e}"))?;
                // A fixed value is checked now, as the library will read it.
                // A value taken from the arrival is not known yet: the key
                // must be declared, and its type is checked per message.
                let candidates = match template.render(&Arrival::default(), "", "") {
                    Some(value) if template.is_fixed() => vec![value],
                    _ => vec!["0".to_string(), "x".to_string()],
                };
                let mut refusal = None;
                for value in candidates {
                    match formats.engine.context_from_text(&[(key.clone(), value)]) {
                        Ok(_) => {
                            refusal = None;
                            break;
                        }
                        Err(e) => refusal = Some(e),
                    }
                }
                if let Some(e) = refusal {
                    bail!("route {name}: set: {e}");
                }
                context.push((key.clone(), template));
            }
            let encoding = rc
                .encoding
                .clone()
                .unwrap_or_else(|| default_encoding(to_wire, &rc.to));
            routes.push(Route {
                cfg: rc.clone(),
                from_wire,
                to_wire,
                encoding,
                context,
                counters: Counters::default(),
            });
        }
        // A route whose output another route (or itself) would receive is a
        // loop, or a chain that multiplies one message into several.
        for out in &routes {
            let published = format!("{}**", out.cfg.publish_prefix);
            let published = KeyExpr::try_from(published.as_str())
                .map_err(|e| anyhow!("route {}: publish_prefix: {e}", out.cfg.name))?;
            for inp in &routes {
                let sub =
                    KeyExpr::try_from(inp.cfg.subscribe.as_str()).map_err(|e| anyhow!("{e}"))?;
                if published.intersects(&sub) {
                    bail!(
                        "route {} publishes under `{}`, which route {} subscribes to (`{}`)",
                        out.cfg.name,
                        out.cfg.publish_prefix,
                        inp.cfg.name,
                        inp.cfg.subscribe
                    );
                }
            }
        }
        Ok(Table { formats, routes })
    }

    /// Convert one message of route `idx` received now on `key`, with no
    /// sample metadata. Counts the outcome.
    pub fn convert(&self, idx: usize, key: &str, payload: &[u8]) -> Result<Converted> {
        self.convert_arrival(idx, &Arrival::now(key), payload)
    }

    /// Convert one received message of route `idx`. Counts the outcome.
    pub fn convert_arrival(
        &self,
        idx: usize,
        arrival: &Arrival,
        payload: &[u8],
    ) -> Result<Converted> {
        let route = &self.routes[idx];
        let key = arrival.key.as_str();
        let done = self.convert_inner(route, arrival, payload);
        match &done {
            Ok(_) => {
                route.counters.converted.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                let n = route.counters.refused.fetch_add(1, Ordering::Relaxed) + 1;
                let text = format!("{e:#}");
                // Every refusal is counted; the log carries the 1st, 2nd,
                // 4th, 8th… so a bad feed cannot flood it.
                if n.is_power_of_two() {
                    warn!(route = %route.cfg.name, key, refused = n, error = %text, "message refused");
                }
                if let Ok(mut last) = route.counters.last_error.lock() {
                    *last = Some(text);
                }
            }
        }
        done
    }

    fn convert_inner(&self, route: &Route, arrival: &Arrival, payload: &[u8]) -> Result<Converted> {
        let key = arrival.key.as_str();
        let context: Vec<(String, String)> = route
            .context
            .iter()
            .filter_map(|(k, t)| {
                t.render(arrival, &route.cfg.subscribe, &route.cfg.strip_prefix)
                    .map(|v| (k.clone(), v))
            })
            .collect();
        let tail = key
            .strip_prefix(route.cfg.strip_prefix.as_str())
            .ok_or_else(|| {
                anyhow!(
                    "key `{key}` does not begin with `{}`",
                    route.cfg.strip_prefix
                )
            })?;
        let hex_in;
        let input: &str = if is_binary(route.from_wire) {
            hex_in = hex::encode(payload);
            &hex_in
        } else {
            std::str::from_utf8(payload)
                .map_err(|_| anyhow!("malformed-wire: the payload is not UTF-8 text"))?
        };
        let out = self
            .formats
            .engine
            .convert_text(&route.cfg.from, &route.cfg.to, input, Some(&context))
            .map_err(|e| anyhow!("{e}"))?;
        let payload = if is_binary(route.to_wire) {
            hex::decode(&out)
                .map_err(|e| anyhow!("the engine returned text that is not hex: {e}"))?
        } else {
            out.into_bytes()
        };
        Ok(Converted {
            key: format!("{}{tail}", route.cfg.publish_prefix),
            payload,
            encoding: route.encoding.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Announcement and heartbeat
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct Announce<'a> {
    class: &'a str,
    version: &'a str,
    instance: &'a str,
    site: &'a str,
    capabilities: Vec<&'a str>,
    translates_from: Vec<&'a str>,
    translates_to: Vec<&'a str>,
    config_digest: &'a str,
    library_rev: &'a str,
}

#[derive(Serialize)]
struct RouteHealth<'a> {
    name: &'a str,
    subscribe: &'a str,
    from: &'a str,
    to: &'a str,
    received: u64,
    converted: u64,
    refused: u64,
    dropped: u64,
    publish_failed: u64,
    last_error: Option<String>,
}

#[derive(Serialize)]
struct Health<'a> {
    class: &'a str,
    instance: &'a str,
    version: &'a str,
    uptime_s: u64,
    /// `ok`, or `degraded` when any route has refused, dropped or failed to
    /// publish a message since the previous heartbeat.
    state: &'a str,
    routes: Vec<RouteHealth<'a>>,
}

pub fn announce_key(cfg: &Config) -> String {
    format!(
        "{}services/shims/{SHIM_CLASS}/{}/announce",
        cfg.key_prefix, cfg.instance
    )
}

pub fn health_key(cfg: &Config) -> String {
    format!(
        "{}health/shims/{SHIM_CLASS}/{}",
        cfg.key_prefix, cfg.instance
    )
}

fn announce_json(cfg: &Config, table: &Table, config_digest: &str) -> String {
    let uniq = |f: fn(&Route) -> &str| {
        let mut v: Vec<&str> = table.routes.iter().map(f).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    serde_json::to_string(&Announce {
        class: SHIM_CLASS,
        version: env!("CARGO_PKG_VERSION"),
        instance: &cfg.instance,
        site: &cfg.site,
        capabilities: vec!["convert"],
        translates_from: uniq(|r| r.cfg.from.as_str()),
        translates_to: uniq(|r| r.cfg.to.as_str()),
        config_digest,
        library_rev: &table.formats.library_rev,
    })
    .unwrap_or_default()
}

fn health_json(cfg: &Config, table: &Table, started: Instant, faults_before: &mut u64) -> String {
    let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
    let routes: Vec<RouteHealth> = table
        .routes
        .iter()
        .map(|r| RouteHealth {
            name: &r.cfg.name,
            subscribe: &r.cfg.subscribe,
            from: &r.cfg.from,
            to: &r.cfg.to,
            received: load(&r.counters.received),
            converted: load(&r.counters.converted),
            refused: load(&r.counters.refused),
            dropped: load(&r.counters.dropped),
            publish_failed: load(&r.counters.publish_failed),
            last_error: r.counters.last_error.lock().ok().and_then(|g| g.clone()),
        })
        .collect();
    let faults: u64 = routes
        .iter()
        .map(|r| r.refused + r.dropped + r.publish_failed)
        .sum();
    let state = if faults > *faults_before {
        "degraded"
    } else {
        "ok"
    };
    *faults_before = faults;
    serde_json::to_string(&Health {
        class: SHIM_CLASS,
        instance: &cfg.instance,
        version: env!("CARGO_PKG_VERSION"),
        uptime_s: started.elapsed().as_secs(),
        state,
        routes,
    })
    .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The run loop
// ---------------------------------------------------------------------------

pub struct RunOptions {
    /// Samples the queue between the subscribers and the converter holds.
    pub queue_depth: usize,
    pub heartbeat: Duration,
    /// SHA-256 of the route table file, for the announcement.
    pub config_digest: String,
}

/// Subscribe, convert and republish until `shutdown` resolves. Returns an
/// error when the fabric session can no longer carry the heartbeat: the shim
/// then exits and its supervisor restarts it.
pub async fn run(
    session: zenoh::Session,
    cfg: Config,
    table: Arc<Table>,
    opts: RunOptions,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(usize, Arrival, Vec<u8>)>(opts.queue_depth);

    let mut subscribers = Vec::new();
    for (idx, route) in table.routes.iter().enumerate() {
        let tx = tx.clone();
        let table_cb = table.clone();
        let sub = session
            .declare_subscriber(route.cfg.subscribe.as_str())
            .callback(move |sample| {
                let counters = &table_cb.routes[idx].counters;
                counters.received.fetch_add(1, Ordering::Relaxed);
                let arrival = Arrival {
                    key: sample.key_expr().to_string(),
                    received_ms: now_ms(),
                    encoding: sample.encoding().to_string(),
                    attachment: sample.attachment().map(|a| a.to_bytes().to_vec()),
                    timestamp_ms: sample.timestamp().and_then(|t| {
                        t.get_time()
                            .to_system_time()
                            .duration_since(std::time::UNIX_EPOCH)
                            .ok()
                            .map(|d| d.as_millis() as i64)
                    }),
                };
                let item = (idx, arrival, sample.payload().to_bytes().to_vec());
                // Never wait here: a subscriber that stops draining can stall
                // the router's delivery to every peer.
                if tx.try_send(item).is_err() {
                    counters.dropped.fetch_add(1, Ordering::Relaxed);
                }
            })
            .await
            .map_err(|e| anyhow!("route {}: declaring the subscriber: {e}", route.cfg.name))?;
        info!(route = %route.cfg.name, subscribe = %route.cfg.subscribe, from = %route.cfg.from,
              to = %route.cfg.to, publish_prefix = %route.cfg.publish_prefix, "route up");
        subscribers.push(sub);
    }
    drop(tx);

    session
        .put(
            announce_key(&cfg),
            announce_json(&cfg, &table, &opts.config_digest),
        )
        .encoding(Encoding::APPLICATION_JSON)
        .await
        .map_err(|e| anyhow!("publishing the announcement: {e}"))?;

    let started = Instant::now();
    let mut faults_before = 0u64;
    let mut beat = tokio::time::interval(opts.heartbeat);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("shutdown");
                return Ok(());
            }
            _ = beat.tick() => {
                let body = health_json(&cfg, &table, started, &mut faults_before);
                session
                    .put(health_key(&cfg), body)
                    .encoding(Encoding::APPLICATION_JSON)
                    .await
                    .map_err(|e| anyhow!("publishing the heartbeat: {e}"))?;
            }
            item = rx.recv() => {
                let Some((idx, arrival, payload)) = item else { bail!("every subscriber closed") };
                let Ok(out) = table.convert_arrival(idx, &arrival, &payload) else { continue };
                let put = session
                    .put(out.key.as_str(), out.payload)
                    .encoding(Encoding::from(out.encoding.as_str()))
                    .await;
                if let Err(e) = put {
                    let counters = &table.routes[idx].counters;
                    let n = counters.publish_failed.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_power_of_two() {
                        error!(route = %table.routes[idx].cfg.name, key = %out.key,
                               failed = n, error = %e, "publish failed");
                    }
                    if let Ok(mut last) = counters.last_error.lock() {
                        *last = Some(format!("publish: {e}"));
                    }
                }
            }
        }
    }
}
