// SPDX-License-Identifier: Apache-2.0
//! The key every snapshot is published on: `<prefix>/<site>/<node>`.
//!
//! The prefix is a setting (`--key-prefix`, `TOPOLOGY_KEY_PREFIX`), because a
//! fabric's key space is its own: one fabric keeps operational data under
//! `ops/`, another under `@sys/` or a team name, and an access-control rule
//! has to name the key the publisher writes. The default is
//! `ops/fabric/topology`, which is what every deployment before the setting
//! used.
//!
//! Everything is checked at start, so a bad value stops the process with a
//! message rather than publishing on a key the surface never subscribes to.

/// The prefix used when none is given.
pub const DEFAULT_PREFIX: &str = "ops/fabric/topology";

/// Characters a site or node may not contain: the chunk separator, and the
/// characters zenoh gives a meaning in key expressions (`*` and `**`
/// wildcards, `$*` sub-chunk wildcards, and `#` and `?`, which are not allowed
/// in a key expression at all).
const FORBIDDEN: [char; 5] = ['/', '*', '$', '#', '?'];

/// Check one chunk (a site or a node).
pub fn check_chunk(what: &str, v: &str) -> anyhow::Result<()> {
    if v.is_empty() {
        anyhow::bail!("{what} is empty");
    }
    if let Some(c) = v.chars().find(|c| FORBIDDEN.contains(c)) {
        anyhow::bail!(
            "{what} {v:?} contains {c:?}; a {what} is one key-expression chunk and may not \
             contain any of / * $ # ?"
        );
    }
    Ok(())
}

/// Check a prefix: one or more chunks joined by `/`, no wildcard, no empty
/// chunk (so no leading, trailing or doubled `/`).
pub fn check_prefix(p: &str) -> anyhow::Result<()> {
    if p.is_empty() {
        anyhow::bail!("--key-prefix is empty");
    }
    for chunk in p.split('/') {
        if chunk.is_empty() {
            anyhow::bail!(
                "--key-prefix {p:?} has an empty chunk (a leading, trailing or doubled /)"
            );
        }
        check_chunk("--key-prefix chunk", chunk)?;
    }
    Ok(())
}

/// The key one router's snapshot is published on.
pub fn router_key(prefix: &str, site: &str, node: &str) -> String {
    format!("{prefix}/{site}/{node}")
}

/// What `serve` subscribes to: every router, or one site's.
pub fn selector(prefix: &str, site: Option<&str>) -> String {
    match site {
        Some(s) => format!("{prefix}/{s}/**"),
        None => format!("{prefix}/**"),
    }
}

/// `<site>/<node>` from a received key, which the store uses as the router's
/// identity. A key outside the prefix is kept whole.
pub fn router_id<'a>(prefix: &str, key: &'a str) -> &'a str {
    key.strip_prefix(prefix)
        .and_then(|r| r.strip_prefix('/'))
        .unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_key_every_deployment_already_uses() {
        assert_eq!(
            router_key(DEFAULT_PREFIX, "site-a", "core-1"),
            "ops/fabric/topology/site-a/core-1"
        );
        assert_eq!(selector(DEFAULT_PREFIX, None), "ops/fabric/topology/**");
        assert_eq!(
            selector(DEFAULT_PREFIX, Some("site-a")),
            "ops/fabric/topology/site-a/**"
        );
    }

    #[test]
    fn a_custom_prefix_is_used_on_both_sides() {
        let p = "demo/topology";
        check_prefix(p).unwrap();
        let k = router_key(p, "lab", "r1");
        assert_eq!(k, "demo/topology/lab/r1");
        assert_eq!(router_id(p, &k), "lab/r1");
        assert_eq!(selector(p, None), "demo/topology/**");
    }

    #[test]
    fn a_prefix_is_not_stripped_from_a_longer_chunk() {
        // "ops/fabric/topology2/..." is not under "ops/fabric/topology".
        assert_eq!(
            router_id("ops/fabric/topology", "ops/fabric/topology2/a/b"),
            "ops/fabric/topology2/a/b"
        );
    }

    #[test]
    fn bad_values_are_refused_with_the_reason() {
        for bad in [
            "", "/ops", "ops/", "ops//x", "ops/*", "ops/**", "a$*b", "a#b", "a?b",
        ] {
            assert!(check_prefix(bad).is_err(), "{bad:?} should be refused");
        }
        for bad in ["", "a/b", "*", "a$b", "a#", "a?"] {
            assert!(
                check_chunk("site", bad).is_err(),
                "{bad:?} should be refused"
            );
        }
        check_chunk("node", "core-1.example").unwrap();
        let e = check_chunk("node", "a/b").unwrap_err().to_string();
        assert!(e.contains("node") && e.contains("'/'"), "{e}");
    }
}
