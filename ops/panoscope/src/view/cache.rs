// SPDX-License-Identifier: Apache-2.0
//
// One assembly per generation, shared by every viewer asking the same thing.
//
// Assembly is per viewer because scope and parameters are, but most viewers
// ask the same question: the default parameters at full scope. On a large
// goatnet one `/api/view` costs enough on the mgmt host that ten viewers polling
// every 1.5 s spend more than a core
// re-deriving the same bytes. A body is cached under everything the answer
// depends on — generation, scope, parameters, and the minute for a windowed
// view — so it can only be served to a request that would have produced it.
//
// THE KEY IS THOSE VALUES THEMSELVES, NOT THE ENTITY TAG. The tag is a 64-bit
// unkeyed FNV of the same inputs, which is fine for what a tag does: a
// collision costs one client a spurious 304 on its own request. As a cache key
// it would decide WHICH VIEWER'S BODY a request receives, and assembly cuts the
// body to the viewer's scope (ADR 1073). The parameters are client-supplied
// strings and FNV-1a is invertible, so a scoped participant could construct a
// request whose tag equals the full-scope viewer's and be handed every
// organisation's topic keys. `ViewKey` compares the full strings, so two
// requests share a body only when they are the same question.
//
// The map is dropped whole when the generation moves, so a stale body cannot
// outlive the model it came from.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Most distinct (scope, parameters) answers held for one generation. Beyond
/// this the map is cleared rather than grown: the common case is a handful of
/// distinct questions, and an unbounded map would hold megabytes per viewer.
const MAX_ENTRIES: usize = 256;

/// What one answer depends on besides the model generation. Built by
/// `assemble::view_key` beside `assemble::etag`, from the same inputs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViewKey {
    /// The wall-clock minute for a windowed view, whose content moves with
    /// time; 0 for an unwindowed one.
    pub minute: u64,
    /// `ViewerScope::cache_key()`.
    pub scope: String,
    /// The parameters as the JSON they serialise to.
    pub params: String,
}

type Cache = Mutex<(u64, HashMap<ViewKey, Arc<str>>)>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new((0, HashMap::new())))
}

/// The serialised body for `key`, building it with `build` on a miss.
///
/// `build` runs OUTSIDE the lock: two viewers arriving together may both
/// build, which costs one extra assembly, where holding the lock across a
/// 200 ms build would stall every other request in the process.
///
/// A failed build is returned and NEVER inserted: a cached failure would be
/// served to every viewer asking the same question until the generation moved.
pub fn body_for<E>(
    generation: u64,
    key: ViewKey,
    build: impl FnOnce() -> Result<String, E>,
) -> Result<Arc<str>, E> {
    if let Some(hit) = lookup(generation, &key) {
        return Ok(hit);
    }
    let body: Arc<str> = Arc::from(build()?);
    if let Ok(mut c) = cache().lock() {
        if c.0 != generation {
            c.0 = generation;
            c.1.clear();
        }
        if c.1.len() >= MAX_ENTRIES {
            c.1.clear();
        }
        c.1.insert(key, body.clone());
    }
    Ok(body)
}

fn lookup(generation: u64, key: &ViewKey) -> Option<Arc<str>> {
    let mut c = cache().lock().ok()?;
    if c.0 != generation {
        c.0 = generation;
        c.1.clear();
        return None;
    }
    c.1.get(key).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn key(scope: &str, params: &str) -> ViewKey {
        ViewKey { minute: 0, scope: scope.into(), params: params.into() }
    }

    // The cache is one process-wide static, and a lookup at a new generation
    // clears it. Tests run in parallel, so without this lock one test's
    // generation would empty the map under another's assertions.
    static SERIAL: Mutex<()> = Mutex::new(());
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn one_build_per_question_per_generation_and_none_survives_it() {
        let _serial = serial();
        let builds = AtomicUsize::new(0);
        let mut build = || {
            builds.fetch_add(1, Ordering::SeqCst);
            Ok::<_, Infallible>("{\"body\":1}".to_string())
        };
        let a = body_for(7, key("full", "{\"a\":1}"), &mut build).unwrap();
        let b = body_for(7, key("full", "{\"a\":1}"), &mut build).unwrap();
        assert_eq!(&*a, &*b);
        assert_eq!(builds.load(Ordering::SeqCst), 1, "the second viewer was served the first's bytes");

        // A different question in the same generation is its own answer.
        body_for(7, key("full", "{\"b\":1}"), &mut build).unwrap();
        assert_eq!(builds.load(Ordering::SeqCst), 2);

        // The model moved: nothing from the old generation may be served.
        body_for(8, key("full", "{\"a\":1}"), &mut build).unwrap();
        assert_eq!(builds.load(Ordering::SeqCst), 3);
        assert!(lookup(8, &key("full", "{\"b\":1}")).is_none(), "the previous generation's answers are gone");
    }

    #[test]
    fn a_body_is_never_served_across_scopes() {
        let _serial = serial();
        // Same generation, same parameters, same minute: only the scope
        // differs. Whatever the two requests' entity tags hash to, the scoped
        // viewer must get a body built for the scoped viewer.
        let full = body_for(21, key("full", "{}"), || Ok::<_, Infallible>("FULL".to_string())).unwrap();
        let scoped = body_for(21, key("scoped:false:org-a", "{}"), || {
            Ok::<_, Infallible>("SCOPED".to_string())
        })
        .unwrap();
        assert_eq!(&*full, "FULL");
        assert_eq!(&*scoped, "SCOPED");
        // And a windowed view's minute is part of the question too.
        let later = ViewKey { minute: 1, ..key("full", "{}") };
        let next = body_for(21, later, || Ok::<_, Infallible>("FULL+1m".to_string())).unwrap();
        assert_eq!(&*next, "FULL+1m");
    }

    #[test]
    fn a_failed_build_is_returned_and_never_cached() {
        let _serial = serial();
        let failed: Result<Arc<str>, &str> = body_for(33, key("full", "{}"), || Err("nan"));
        assert_eq!(failed.unwrap_err(), "nan");
        assert!(lookup(33, &key("full", "{}")).is_none(), "a failure was cached");
        // The next viewer builds for themselves and gets a real body.
        let ok = body_for(33, key("full", "{}"), || Ok::<_, &str>("{\"ok\":1}".to_string())).unwrap();
        assert_eq!(&*ok, "{\"ok\":1}");
    }

    #[test]
    fn the_map_does_not_grow_without_bound() {
        let _serial = serial();
        for i in 0..(MAX_ENTRIES + 10) {
            body_for(99, key("full", &format!("{{\"i\":{i}}}")), || Ok::<_, Infallible>("{}".to_string())).unwrap();
        }
        let held = cache().lock().unwrap().1.len();
        assert!(held <= MAX_ENTRIES, "{held} entries held");
    }
}
