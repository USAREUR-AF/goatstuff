// Persisted derived state, so a restart does not re-derive it from scratch.
//
// WHAT THIS IS FOR, and what it deliberately is NOT. F-366 established that the
// audit replay's cost is in its INPUT, not its output: 32,876,051 events parsed
// to derive 166 unique peers. Streaming fixed the memory half of that. This
// fixes the other half — the 136,830 MinIO object fetches and 191 seconds spent
// re-deriving, on every cold start, a result that is a few hundred kilobytes.
//
// So the thing persisted here is the DERIVED state (topology + flow), never the
// events. Caching the events would be caching an input nothing reads twice.
//
// THE STALENESS RULE IS THE WHOLE CORRECTNESS ARGUMENT. A snapshot lets us
// replay a shorter window, and a window shorter than the gap since the snapshot
// was written would silently lose whatever happened in between. So the replay
// window becomes `age_of_snapshot + margin`, and if that is not smaller than
// the configured full window there is nothing to gain and we do the full walk
// anyway. A snapshot can only ever SHRINK work, never skip a period.
//
// Every failure mode lands on "ignore it and do the full replay": missing file,
// unreadable file, malformed JSON, a version this build does not know, or a
// snapshot older than the full window. A snapshot is an optimisation, and an
// optimisation that can wedge a cold start is worse than no optimisation.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::model::{EdgeKey, EdgeState, FlowState, Topology};

/// Bumped whenever Topology or FlowState change shape. A snapshot written by a
/// different version is ignored rather than deserialized on a hope — a partial
/// or field-shifted decode would poison the registry with plausible garbage,
/// which is far worse than a slow start.
///
/// 2: FlowState is written as sequences of pairs (`FlowStateDisk`). Version 1
/// could never be written, so no version-1 file exists to migrate.
pub const SNAPSHOT_VERSION: u32 = 2;

/// Extra hours of replay on top of the measured snapshot age. Covers clock
/// skew, the write lagging the data it describes, and the hour-granularity of
/// the audit prefixes the walker lists.
const STALENESS_MARGIN_HOURS: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub version: u32,
    /// Unix millis when this was written.
    pub written_at_ms: u64,
    pub topology: Topology,
    pub flow: FlowState,
}

/// FlowState as it is written to disk, which is not how it is written to the
/// API.
///
/// `FlowState.edges` is a `HashMap<EdgeKey, EdgeState>` and `EdgeKey` is a
/// struct. JSON object keys must be strings, so `serde_json` refused the whole
/// value with `key must be a string`, and every snapshot write failed. A failed
/// write is logged-and-continue by design, so nothing else noticed.
///
/// `#[serde(with = ...)]` on the field would also change the HTTP surface that
/// serialises `FlowState`, so the on-disk form is its own type: maps become
/// sequences of pairs, which JSON represents natively.
#[derive(Debug, Serialize, Deserialize)]
struct FlowStateDisk {
    edges: Vec<(EdgeKey, EdgeState)>,
    sampling_drops: Vec<(String, u64)>,
}

impl From<&FlowState> for FlowStateDisk {
    fn from(f: &FlowState) -> Self {
        FlowStateDisk {
            edges: f.edges.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            sampling_drops: f
                .sampling_drops
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        }
    }
}

impl From<FlowStateDisk> for FlowState {
    fn from(d: FlowStateDisk) -> Self {
        let mut f = FlowState::default();
        f.edges.extend(d.edges);
        f.sampling_drops.extend(d.sampling_drops);
        f
    }
}

/// The literal on-disk document. Kept separate from `StateSnapshot` so the
/// in-memory type the rest of the process uses never has to care.
#[derive(Debug, Serialize, Deserialize)]
struct SnapshotDoc {
    version: u32,
    written_at_ms: u64,
    topology: Topology,
    flow: FlowStateDisk,
}

/// Read a snapshot, or `None` for any reason at all. Never returns an error:
/// the caller's fallback is always "replay the full window", and forcing them
/// to handle six failure modes that all mean the same thing invites one of them
/// being handled wrongly.
pub fn load(path: &Path) -> Option<StateSnapshot> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!(path = %path.display(), "state snapshot: none on disk — full replay");
            return None;
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e,
                  "state snapshot: unreadable — falling back to full replay");
            return None;
        }
    };
    // Version first, from a loose parse: a document from another build may not
    // match this build's shape at all, and that is a version mismatch, not a
    // malformed file.
    let found_version = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.get("version").and_then(|n| n.as_u64()));
    if let Some(found) = found_version {
        if found != u64::from(SNAPSHOT_VERSION) {
            warn!(
                found,
                expected = SNAPSHOT_VERSION,
                "state snapshot: version mismatch — falling back to full replay"
            );
            return None;
        }
    }
    let doc: SnapshotDoc = match serde_json::from_slice(&bytes) {
        Ok(s) => s,
        Err(e) => {
            warn!(path = %path.display(), error = %e,
                  "state snapshot: malformed — falling back to full replay");
            return None;
        }
    };
    Some(StateSnapshot {
        version: doc.version,
        written_at_ms: doc.written_at_ms,
        topology: doc.topology,
        flow: doc.flow.into(),
    })
}

/// Write a snapshot atomically: temp file in the same directory, then rename.
/// A half-written snapshot that survives a crash would be indistinguishable
/// from a good one at the next boot, and `load` would only catch it if the
/// truncation happened to break the JSON.
pub fn save(path: &Path, topology: &Topology, flow: &FlowState, now_ms: u64) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let doc = SnapshotDoc {
        version: SNAPSHOT_VERSION,
        written_at_ms: now_ms,
        topology: topology.clone(),
        flow: FlowStateDisk::from(flow),
    };
    let bytes = serde_json::to_vec(&doc)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp: PathBuf = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// How many hours the replay must cover given a snapshot written at
/// `written_at_ms`, or `None` when the snapshot cannot shrink the work.
///
/// `None` means "do the full `full_window_hours` walk" and is returned both for
/// a snapshot from the future (clock went backwards — do not trust it to bound
/// a window) and for one old enough that the delta is no smaller than the full
/// window.
pub fn replay_window_hours(written_at_ms: u64, now_ms: u64, full_window_hours: u32) -> Option<u32> {
    if written_at_ms > now_ms {
        return None;
    }
    let age_hours = ((now_ms - written_at_ms) as f64 / 3_600_000.0).ceil() as u32;
    let needed = age_hours.saturating_add(STALENESS_MARGIN_HOURS);
    if needed >= full_window_hours {
        None
    } else {
        Some(needed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3_600_000;

    #[test]
    fn a_fresh_snapshot_shrinks_the_window_to_age_plus_margin() {
        let now = 1_000 * HOUR;
        let written = now - 3 * HOUR;
        assert_eq!(replay_window_hours(written, now, 168), Some(3 + STALENESS_MARGIN_HOURS));
    }

    /// The margin exists so a snapshot written moments ago still replays a
    /// couple of hours — the walker lists by hour prefix, and the write lags
    /// the data it describes.
    #[test]
    fn a_just_written_snapshot_still_replays_the_margin() {
        let now = 1_000 * HOUR;
        assert_eq!(replay_window_hours(now, now, 168), Some(STALENESS_MARGIN_HOURS));
    }

    /// THE CORRECTNESS BOUNDARY: once the delta is not smaller than the full
    /// window there is nothing to gain, and shrinking further would skip a
    /// period nobody covered.
    #[test]
    fn a_stale_snapshot_falls_back_to_the_full_window() {
        let now = 1_000 * HOUR;
        let written = now - 200 * HOUR;
        assert_eq!(replay_window_hours(written, now, 168), None);
    }

    #[test]
    fn a_snapshot_exactly_at_the_window_edge_falls_back() {
        let now = 1_000 * HOUR;
        let written = now - 166 * HOUR; // 166 + 2 margin == 168
        assert_eq!(replay_window_hours(written, now, 168), None);
    }

    /// A clock that went backwards must not be trusted to bound a window.
    #[test]
    fn a_future_snapshot_falls_back_to_the_full_window() {
        let now = 1_000 * HOUR;
        assert_eq!(replay_window_hours(now + 5 * HOUR, now, 168), None);
    }

    #[test]
    fn missing_file_loads_as_none() {
        let p = std::env::temp_dir().join("panoscope-snapshot-does-not-exist.json");
        let _ = std::fs::remove_file(&p);
        assert!(load(&p).is_none());
    }

    #[test]
    fn malformed_file_loads_as_none_rather_than_erroring() {
        let p = std::env::temp_dir().join("panoscope-snapshot-malformed.json");
        std::fs::write(&p, b"{ not json").unwrap();
        assert!(load(&p).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_version_from_another_build_is_ignored() {
        let p = std::env::temp_dir().join("panoscope-snapshot-badversion.json");
        let body = format!(
            r#"{{"version":{},"written_at_ms":1,"topology":{{}},"flow":{{}}}}"#,
            SNAPSHOT_VERSION + 1
        );
        std::fs::write(&p, body).unwrap();
        assert!(load(&p).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_saved_snapshot_round_trips() {
        let p = std::env::temp_dir().join("panoscope-snapshot-roundtrip.json");
        let _ = std::fs::remove_file(&p);
        let topo = Topology::default();
        let flow = FlowState::default();
        save(&p, &topo, &flow, 12_345).expect("save");
        let got = load(&p).expect("load");
        assert_eq!(got.version, SNAPSHOT_VERSION);
        assert_eq!(got.written_at_ms, 12_345);
        let _ = std::fs::remove_file(&p);
    }

    /// The round-trip above uses an empty FlowState, and an empty map
    /// serialises whatever its key type is, so it passed while every real
    /// write failed. This one carries edges, drops and a populated topology.
    #[test]
    fn a_snapshot_with_real_flow_state_round_trips() {
        let p = std::env::temp_dir().join("panoscope-snapshot-nonempty.json");
        let _ = std::fs::remove_file(&p);
        let mut flow = FlowState::default();
        flow.record_event("zidA".into(), "acme/telemetry".into(), 1_000, 5.0);
        flow.record_event("zidB".into(), "acme/status".into(), 2_000, 5.0);
        flow.record_drop("acme/telemetry".into());
        let mut topo = Topology::default();
        topo.record_publish_declaration("zidA".into(), "acme/telemetry".into(), 1_000);
        topo.record_subscribe_declaration("zidC".into(), "acme/**".into(), 1_000);

        save(&p, &topo, &flow, 12_345).expect("save a non-empty snapshot");
        let got = load(&p).expect("load it back");
        let _ = std::fs::remove_file(&p);

        assert_eq!(got.written_at_ms, 12_345);
        assert_eq!(got.flow.edges.len(), 2);
        let key = EdgeKey::new("zidA".into(), "acme/telemetry".into());
        let edge = got.flow.edges.get(&key).expect("edge survives");
        assert_eq!(edge.total_events, flow.edges[&key].total_events);
        assert_eq!(got.flow.sampling_drops.get("acme/telemetry"), Some(&1));
        assert_eq!(got.topology.peers.len(), topo.peers.len());
    }
}
