// SPDX-License-Identifier: Apache-2.0
//! Static assets, compiled in and served with cache validators.
//!
//! # Why these are files and not one blob
//!
//! The page used to be a single `index.html` carrying markup, every CSS rule and
//! every line of behaviour, with the design system spliced in ahead of it on
//! each request. That is one artefact doing three jobs, and it made the whole
//! bundle uncacheable: a one-word copy edit re-sent the design system too.
//!
//! Split, each asset has its own lifetime and its own ETag. The design system in
//! particular is bytes this surface does not own — it belongs to
//! `docs/design-system/`, is `include_str!`d from there rather than vendored,
//! and is served at its own URL exactly as `design-system.md` §"How consumers
//! receive the CSS bytes" describes.
//!
//! # Caching
//!
//! Everything here is fixed at image build time, so an asset's bytes cannot
//! change without a new image. The ETag is a hash of the content: a reload after
//! a redeploy fetches, a reload without one gets `304` and no body.
//!
//! Deliberately NOT `immutable` with a long `max-age`: these are served from a
//! mesh-internal host an operator may reach mid-incident, and a stale bundle
//! that cannot be forced out with a refresh is a bad trade for bytes we are not
//! short of.

use std::collections::HashMap;
use std::sync::OnceLock;

/// One compiled-in asset.
pub struct Asset {
    pub body: &'static str,
    pub content_type: &'static str,
    /// Strong ETag over the content. Computed once, at first use.
    pub etag: String,
}

/// FNV-1a. Enough to tell two builds apart, which is all an ETag has to do;
/// this is a cache validator, never a security boundary.
fn hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Every asset is listed explicitly. `include_str!` needs a literal path, and
/// an explicit table also means a file that is added but not wired shows up as
/// a 404 in review rather than a mystery at runtime.
const FILES: &[(&str, &str, &str)] = &[
    ("/", include_str!("../static/index.html"), HTML),
    ("/assets/app.css", include_str!("../static/app.css"), CSS),
    ("/assets/app.js", include_str!("../static/app.js"), JS),
    ("/assets/lib.js", include_str!("../static/lib.js"), JS),
    (
        "/assets/render/verdict.js",
        include_str!("../static/render/verdict.js"),
        JS,
    ),
    (
        "/assets/render/graph.js",
        include_str!("../static/render/graph.js"),
        JS,
    ),
    (
        "/assets/render/findings.js",
        include_str!("../static/render/findings.js"),
        JS,
    ),
    (
        "/assets/render/feed.js",
        include_str!("../static/render/feed.js"),
        JS,
    ),
    (
        "/assets/render/routers.js",
        include_str!("../static/render/routers.js"),
        JS,
    ),
    (
        "/assets/render/planner.js",
        include_str!("../static/render/planner.js"),
        JS,
    ),
];

const HTML: &str = "text/html; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";

/// The design system, from its canonical home. Not vendored — a change there
/// reaches this surface on the next image build.
const DS_TOKENS: &str = include_str!("../../../docs/design-system/colors_and_type.css");
const DS_COMPONENTS: &str = include_str!("../../../docs/design-system/components.css");

fn table() -> &'static HashMap<&'static str, Asset> {
    static T: OnceLock<HashMap<&'static str, Asset>> = OnceLock::new();
    T.get_or_init(|| {
        // The design system is two files upstream and one asset here: they are
        // always fetched together, and a single request beats two.
        static DS: OnceLock<String> = OnceLock::new();
        let ds = DS.get_or_init(|| format!("{DS_TOKENS}\n{DS_COMPONENTS}\n"));

        let mut m = HashMap::new();
        for (path, body, ct) in FILES.iter().copied().chain(std::iter::once((
            "/assets/design-system.css",
            ds.as_str(),
            CSS,
        ))) {
            m.insert(
                path,
                Asset {
                    body,
                    content_type: ct,
                    etag: format!("\"{:016x}\"", hash(body.as_bytes())),
                },
            );
        }
        m
    })
}

pub fn get(path: &str) -> Option<&'static Asset> {
    table().get(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_asset_is_present_and_non_empty() {
        for (p, _, _) in FILES {
            let a = get(p).unwrap_or_else(|| panic!("{p} missing"));
            assert!(!a.body.is_empty(), "{p} is empty");
            assert!(a.etag.len() > 2, "{p} has no etag");
        }
        assert!(get("/assets/design-system.css").is_some());
    }

    /// A module the page imports but the table does not serve is a 404 at
    /// runtime and nothing at build time. Walk the import graph instead.
    #[test]
    fn every_module_the_page_imports_is_served() {
        for (path, body, ct) in FILES {
            if *ct != JS {
                continue;
            }
            for line in body.lines() {
                let Some(rest) = line.trim().strip_prefix("import ") else {
                    continue;
                };
                let Some(spec) = rest.split(" from ").nth(1) else {
                    continue;
                };
                let spec = spec.trim().trim_end_matches(';').trim_matches('"');
                // Resolve the relative specifier against this module's URL.
                let base = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                let mut resolved = String::from(base);
                for seg in spec.split('/') {
                    match seg {
                        "." => {}
                        ".." => {
                            resolved.truncate(resolved.rfind('/').unwrap_or(0));
                        }
                        s => {
                            resolved.push('/');
                            resolved.push_str(s);
                        }
                    }
                }
                assert!(
                    get(&resolved).is_some(),
                    "{path} imports {spec} -> {resolved}, which nothing serves"
                );
            }
        }
    }

    #[test]
    fn the_design_system_is_served_not_inlined() {
        // The page must LINK the tokens, not carry a copy: one source of truth,
        // and a copy would go stale against docs/design-system/.
        let html = get("/").unwrap().body;
        assert!(
            html.contains("/assets/design-system.css"),
            "index.html must link the design system"
        );
        assert!(
            !html.contains("--bg-page:"),
            "index.html must not carry a copy of the tokens"
        );
        let ds = get("/assets/design-system.css").unwrap().body;
        assert!(
            ds.contains("--bg-page:"),
            "tokens must be in the served asset"
        );
        assert!(
            ds.contains(".chip"),
            "components must be in the served asset"
        );
    }

    #[test]
    fn etags_differ_between_assets_and_are_stable() {
        let a = get("/assets/app.css").unwrap();
        let b = get("/assets/app.js").unwrap();
        assert_ne!(a.etag, b.etag);
        assert_eq!(
            a.etag,
            get("/assets/app.css").unwrap().etag,
            "must be stable"
        );
    }

    /// The page is a well-formed document, not a bag of bytes with the right
    /// words in it.
    ///
    /// This exists because an edit once PREPENDED a JavaScript function to the
    /// top of index.html — the file began `function findings(d) {` — and every
    /// test still passed, because they only asserted that certain strings were
    /// present. The browser then rendered the function body as markup.
    #[test]
    fn the_page_is_structurally_a_document() {
        let raw = get("/").unwrap().body;
        assert!(
            raw.starts_with("<!doctype html>"),
            "index.html must begin with its doctype, not with content — it began: {:?}",
            &raw[..raw.len().min(60)]
        );
        assert_eq!(
            raw.matches("<script").count(),
            1,
            "exactly one script tag; behaviour lives in app.js"
        );
        assert!(
            raw.contains("<script type=\"module\" src=\"/assets/app.js\""),
            "the page must load behaviour as a module, not inline it"
        );
        assert!(
            !raw.contains("function "),
            "no inline behaviour belongs in the markup"
        );
    }

    #[test]
    fn nothing_reaches_outside_the_mesh() {
        // A mesh-only admin surface must not attempt egress on page load — the
        // design system states that as canon, having found a Google Fonts
        // @import that could never have worked on a mesh-only surface.
        for p in ["/", "/assets/app.css", "/assets/app.js"] {
            let body = get(p).unwrap().body;
            for probe in ["http://", "https://", "//fonts.", "cdn."] {
                assert!(
                    !body.contains(probe),
                    "{p} references an external origin ({probe})"
                );
            }
        }
    }
}
