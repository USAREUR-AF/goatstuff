// SPDX-License-Identifier: Apache-2.0
//
// Server-rendered Tier 1 page. Composes the canonical Block 91
// design-system tokens (colors_and_type.css + components.css) with
// the flashlight-specific layout CSS, and inlines the vanilla JS
// client that opens an EventSource against `/events`.
//
// Tier 1 budget: ≤200 KB first-paint per
// docs/design/minimalism-under-network-constraints.md. The design
// system tokens compress well under Brotli (the CompressionLayer
// wired in main.rs negotiates per request); current uncompressed
// surface is well under budget.
//
// Architecture per docs/design/flashlight.md §"Tier 1 surfaces":
//   - Header bar — focus echo + connection-status pill + panoscope
//     deep-link
//   - Canvas 2D sparkline — rolling 60-second event-rate window
//   - Event table — last N=100 (configurable via ?n=, capped at 500
//     in the JS layer)
//   - Filter controls — for v1, the URL bar IS the filter UI;
//     PR #5 of Phase C grows the inline form
//   - Sample payload viewer — row-click expansion deferred to a
//     follow-on alongside the payload-fetch endpoint

use html_escape::encode_safe;

const DESIGN_TOKENS_CSS: &str =
    include_str!("../../../docs/design-system/colors_and_type.css");
const DESIGN_COMPONENTS_CSS: &str =
    include_str!("../../../docs/design-system/components.css");
const FLASHLIGHT_CSS: &str = include_str!("assets/flashlight.css");
const APP_JS: &str = include_str!("assets/app.js");

/// Speculation Rules block — prerender sibling URLs that swap a
/// single filter parameter. Per
/// docs/design/minimalism-under-network-constraints.md Principle 9
/// (MPA navigation, prerendered transitions). Limited to GET
/// requests to `/` with the same origin; Chromium-only feature today,
/// no-op on Firefox/Safari (graceful degradation by construction).
const SPECULATION_RULES_JSON: &str = r#"{
  "prerender": [
    {
      "source": "list",
      "urls": []
    }
  ]
}"#;

/// Build the initial page HTML.
///
/// `query_string` is the raw URL query (without leading `?`) — used
/// to populate the `<title>` and inline a static focus echo as a
/// progressive-enhancement no-script fallback. The JS overrides the
/// fallback with the live-updating echo + connection pill.
///
/// Sibling-surface origins for the Block 94 cross-surface deep-link
/// contract. Each field is the origin URL of a sibling, or empty when
/// not configured at this deployment (the corresponding header button
/// is then suppressed — URL-only contract, no runtime coupling). See
/// `docs/design/zenoh-surface-deep-links.md`.
#[derive(Clone, Default)]
pub struct SiblingOrigins {
    pub panoscope: String,
    pub stethoscope: String,
}

/// Display-only lexicon — the goatnet-appropriate participant noun so one
/// flashlight image renders correct wording on every goatnet (nova-a
/// "vendor"; ZAGROS "org"). Mirror of the portal/panoscope lexicon
/// (`docs/design/portal-lexicon.md`): render-time substitution only — the
/// `?vendor=` query param, the `vendor`/`key` filter kind, the portal
/// `list_vendors` call, and the cert-bound anchor are all untouched; only
/// the word shown changes. Server-side strings are substituted here; the
/// inlined `app.js` reads the same values from a `window.GOAT_LEX` global.
/// Defaults reproduce the nova-a wording (byte-stable); ZAGROS overrides via
/// `FLASHLIGHT_LEX_ORG` env wired from the same `portal_lex_org` group_var
/// the portal reads.
#[derive(Clone)]
pub struct Lexicon {
    pub org: String,
    pub orgs: String,
}

impl Default for Lexicon {
    fn default() -> Self {
        Self {
            org: "vendor".to_string(),
            orgs: "vendors".to_string(),
        }
    }
}

impl Lexicon {
    /// Build from env: `FLASHLIGHT_LEX_ORG` (+ optional
    /// `FLASHLIGHT_LEX_ORGS`; plural derives from the singular when
    /// unset). Unset → nova-a default.
    pub fn from_env() -> Self {
        let mut l = Self::default();
        if let Ok(v) = std::env::var("FLASHLIGHT_LEX_ORG") {
            let t = v.trim();
            if !t.is_empty() {
                l.org = t.to_string();
                l.orgs = format!("{t}s");
            }
        }
        if let Ok(v) = std::env::var("FLASHLIGHT_LEX_ORGS") {
            let t = v.trim();
            if !t.is_empty() {
                l.orgs = t.to_string();
            }
        }
        l
    }
}

/// Build the page HTML for the focus declared by `query_string`.
///
/// `siblings` carries the sibling-surface origins for the Block 94
/// cross-surface deep-link contract. The header `panoscope ↗` +
/// `stethoscope ↗` sibling links render conditionally on the matching
/// field being set. Each link preserves the current focus (passes the
/// same query string through unchanged) so the operator stays on the
/// same target after the hop. Hover-title carries the one-sentence
/// explainer (operator UX feedback — original verbose
/// labels `open in panoscope ↗` / `is this healthy? ↗` were swapped
/// for the surface name; the question form misread as a diagnostic
/// assertion).
pub fn render_page(query_string: &str, siblings: &SiblingOrigins, lex: &Lexicon) -> String {
    let title_suffix = if query_string.is_empty() {
        "no slice".to_string()
    } else {
        // Best-effort decode for the title — failures fall back to
        // the raw bytes since `<title>` doesn't care about character
        // shape, only HTML-escape safety.
        truncate(&decode_query_for_title(query_string), 60)
    };

    // Preserve the current focus on the sibling-surface hop so the
    // operator lands on the same target. Empty query string =
    // sibling root (the "no slice" landing on each surface).
    let qs_suffix = if query_string.is_empty() {
        String::new()
    } else {
        format!("?{}", query_string)
    };
    let panoscope_link = if siblings.panoscope.is_empty() {
        String::new()
    } else {
        format!(
            r#"<a class="panoscope-link sibling-link" href="{origin}/{qs}" target="_blank" rel="noopener" title="Open this focus in panoscope — the whole-fabric COP. See where these keys sit in the overall topology, who else is publishing/subscribing.">panoscope ↗</a>"#,
            origin = encode_safe(&siblings.panoscope),
            qs = encode_safe(&qs_suffix),
        )
    };
    // Stethoscope sibling header link (Block 94). Stethoscope's
    // target picker accepts `?key=` / `?vendor=` (Block 94 canon);
    // flashlight's URL grammar already uses both, so passing the
    // query through verbatim lands on the right target without
    // translation when `key=` or `vendor=` are present. Other
    // flashlight-only params (`role=`, `site=`, `format=`,
    // `compute=`) are silently ignored by stethoscope per the
    // contract's "unknown params silently dropped" rule. Label is
    // the surface name (`stethoscope`) rather than the prior
    // question form (`is this healthy?`) per operator feedback
    // — the question form misread as a diagnostic
    // assertion when the actual hop is just to a sibling tool.
    let stethoscope_link = if siblings.stethoscope.is_empty() {
        String::new()
    } else {
        format!(
            r#"<a class="stethoscope-link sibling-link" href="{origin}/{qs}" target="_blank" rel="noopener" title="Open this focus in stethoscope — the per-target diagnostic surface. Asks 'is this target actually working the way the operator expects?' and returns structured pass/warn/fail verdicts per check.">stethoscope ↗</a>"#,
            origin = encode_safe(&siblings.stethoscope),
            qs = encode_safe(&qs_suffix),
        )
    };

    format!(
        r#"<!doctype html>
<html lang="en" data-theme="dark">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>flashlight · {title}</title>
<style>
{tokens}
{components}
{flashlight}
</style>
<script type="speculationrules">
{speculation}
</script>
</head>
<body>
<header class="top">
  <span class="brand">flashlight</span>
  <span class="focus" id="focus-echo" title="The slice you're tailing — the query string from the URL bar. Edit the URL to widen or narrow the focus.">focus · {query_echo}</span>
  <!-- Navigate-anywhere jump box (Phase F). Present on every view so
       the operator can pivot to any topic/vendor without editing the
       URL. The suggestion list is a custom dropdown (#jump-results)
       populated by app.js from GET /api/directory — it does case-
       insensitive SUBSTRING matching in both directions over the full
       key, vendor prefix, AND peer id/handle, so typing a peer id or any
       middle segment of a key surfaces matches (the native <datalist>
       only prefix-matched the whole value, which missed peer-id-anchored
       keys). Submit resolves to /?vendor=<x> when the text exactly
       matches a known vendor prefix, else /?key=<x>. -->
  <form class="jump" id="jump-form" role="combobox" aria-expanded="false" autocomplete="off" title="Jump to any topic, {org}, or peer. Type any part of a key, {org} name, or peer id — matches come from the portal catalog and the live topology. Enter tails the top match.">
    <input id="jump-input" name="jump" placeholder="search topic / {org} / peer…" aria-label="search topic, {org}, or peer" role="searchbox" aria-autocomplete="list" aria-controls="jump-results" />
    <ul id="jump-results" class="jump-results" role="listbox" hidden></ul>
  </form>
  <span class="status-pill reconnecting" id="status-pill" title="SSE channel state. live = receiving events; reconnecting = retrying after a drop; offline = giving up after repeated failures.">connecting</span>
  <!-- Tier 2 augmentation surface — hidden until the analytics module loads.
       app.js's Tier 2 boot block flips this visible on snapshot tick. The
       [T2] prefix is the badging convention from the design doc; the value
       comes from flashlight_analytics::StatsSnapshot::rate_hz. The `title`
       attributes carry the one-sentence explainer the operator sees on
       hover (Block 89 UX polish). -->
  <span class="t2-badge tier-2-only" id="t2-rate-badge" hidden title="Messages per second on the focused slice over a short rolling window. Computed in-browser by the Tier 2 analytics WASM module; mirrors the sparkline below at a single moment.">
    <span class="t2-tag">T2</span>
    <span class="t2-label">rate</span>
    <span class="t2-value" id="t2-rate-value">— Hz</span>
  </span>
  <!-- The z-score + drift chips were removed (Phase F-3):
       drift compared a wire `format` the live event never carried (so
       it was structurally always 0), and the z-score was a weak
       self-referential signal on event rate shown more prominently than
       the data itself. Honest signals only (Principle 11). A real
       anomaly / schema-drift surface returns when it has real inputs —
       see docs/design/flashlight.md §"Tier 2 surfaces". -->
  {panoscope_link}
  {stethoscope_link}
</header>
<main>
  <!-- Payload viewer — pinned at the top of the focused view (Phase F-3d,
       operator UX). While running it auto-follows the newest
       event; pausing (or clicking a row) freezes it on the selected
       sample. Shows the deserialized (decoded) message AND the serialized
       (raw) bytes side-by-side: the `decoded` pane carries protobuf→JSON
       (or UTF-8 text for texty topics) when available; the `raw` pane is
       always the on-wire bytes (hexdump). app.js reveals it in tail mode,
       keeps it hidden in directory mode. -->
  <section class="sample-detail" id="sample-detail" hidden>
    <div class="sample-detail-head">
      <span class="sample-detail-title" id="sample-detail-title" title="Live = following the newest event on this slice; paused = pinned to the row you clicked. Use ⏸ / ▶ or click a row.">payload · live</span>
      <span class="sample-detail-meta" id="sample-detail-meta"></span>
      <button type="button" id="sample-detail-close" class="sample-detail-close" title="Resume following the newest event." hidden>↺ live</button>
    </div>
    <div class="sample-detail-body" id="sample-detail-body">
      <!-- Deserialized pane — protobuf-decoded JSON (or text). Hidden
           when the payload can't be deserialized; the raw pane still
           shows. -->
      <div class="sample-pane" id="sample-decoded-pane" hidden>
        <div class="sample-pane-label" id="sample-decoded-label">decoded</div>
        <pre class="sample-pane-body" id="sample-decoded"></pre>
      </div>
      <!-- Serialized pane — always the raw on-wire bytes. -->
      <div class="sample-pane" id="sample-raw-pane">
        <div class="sample-pane-label" id="sample-raw-label">raw · hex</div>
        <pre class="sample-pane-body" id="sample-raw">waiting for events…</pre>
      </div>
    </div>
  </section>
  <!-- Directory landing (Phase F navigate-anywhere). Shown by app.js
       when the focus is empty (no slice declared) instead of an empty
       event tail; hidden when a focus is active. Populated from
       GET /api/directory — vendors (groups) → their topics, each a
       one-click link into a focused view. The static fallback below is
       replaced by the live catalog on load; if the portal is
       GET /api/directory — vendors (groups) → their topics, each a
       one-click link into a focused view. The static fallback below is
       replaced by the live catalog on load; if the portal is
       unconfigured the operator still gets the jump box + the example. -->
  <section class="directory" id="directory" hidden>
    <div class="directory-head">
      <span class="directory-title">browse the fabric</span>
      <span class="directory-sub" id="directory-sub">pick a {org} or topic to start tailing — or use the jump box above</span>
    </div>
    <div class="directory-body" id="directory-body">
      <p class="empty-state">loading catalog… or search for a {org}, topic, or peer in the jump box above</p>
    </div>
  </section>
  <!-- Sparkline labels carry peak / now stats so the operator can
       read the canvas without an explicit numeric y-axis (Block 89
       UX polish). app.js updates the stats text on each
       sparkline redraw. -->
  <section class="sparkline-wrap" title="Bar height = events/sec in that 1-second bin. Y-axis is auto-scaled to the rolling peak; absolute values are in the label.">
    <div class="label">
      <span>events / sec · rolling 60 s</span>
      <span class="stats" id="sparkline-stats">peak — · now —</span>
    </div>
    <canvas id="sparkline"></canvas>
  </section>
  <!-- Tier 2 augmentation: longer-window bin-counted sparkline sourced
       from flashlight_analytics::SparklineSnapshot.points. Default
       window 5 min × 60 bins (5-sec each), redrawn from the snapshot
       loop's 2-second cadence. Hidden until the Tier 2 module loads. -->
  <section class="sparkline-wrap tier-2-only" id="t2-sparkline-wrap" hidden title="Same shape as the Tier 1 sparkline above but over a longer rolling window. Bin width is the window divided by 60 cells; bar height is events/sec averaged across the bin.">
    <div class="label">
      <span class="t2-tag">T2</span>
      <span>events / sec · rolling <span id="t2-sparkline-window">5 m</span></span>
      <span class="stats" id="t2-sparkline-stats">peak — · now —</span>
    </div>
    <canvas id="t2-sparkline"></canvas>
  </section>
  <!-- One row per message arrival. Same key repeats across rows
       whenever its publisher emits multiple messages — by design.
       Column header carries the per-event hint to disambiguate
       (rows initially read as a topic
       list). Block 89 UX polish. -->
  <section class="events-table">
    <div class="controls">
      <span class="count" id="count-label" title="One row per message arrival on the focused slice. The same key will repeat across rows whenever its publisher emits multiple messages — that's by design, not a duplicate.">events · showing 0 of 100</span>
      <span class="hint" title="Click any row to inspect that message's raw payload. The live tail carries no payload bytes — they're fetched on demand from the bounded server-side ring.">click a row to inspect payload</span>
      <button id="pause-btn" type="button" title="Freeze the live tail so you can read a row without it scrolling away.">⏸ pause</button>
      <button id="clear-btn" type="button" title="Empty the current table; new events keep accumulating.">clear</button>
    </div>
    <table>
      <thead>
        <tr><th title="UTC timestamp on the sample arrival as observed by the flashlight backend.">time (UTC)</th><th title="Zenoh key expression of the message. Same key repeats across rows when the publisher emits multiple messages.">key (per event)</th><th title="Content-classified wire format of the payload: {{ }} JSON object · [*] JSON array · txt text · 0x binary/protobuf.">fmt</th><th title="Wire-bytes payload size of this message.">bytes</th></tr>
      </thead>
      <tbody id="events-tbody">
        <tr><td colspan="4" class="empty-state">waiting for events… if the focus is empty, search for a {org}, topic, or peer in the jump box above</td></tr>
      </tbody>
    </table>
  </section>
</main>
<script>
// Display-only lexicon for app.js-generated labels (directory heading,
// counts, empty-state). Render-time only — never a key/param/anchor.
window.GOAT_LEX = {{ "org": "{lex_org}", "orgs": "{lex_orgs}" }};
</script>
<script>
{app_js}
</script>
</body>
</html>
"#,
        title = encode_safe(&title_suffix),
        tokens = DESIGN_TOKENS_CSS,
        components = DESIGN_COMPONENTS_CSS,
        flashlight = FLASHLIGHT_CSS,
        speculation = SPECULATION_RULES_JSON,
        query_echo = encode_safe(query_string),
        panoscope_link = panoscope_link,
        stethoscope_link = stethoscope_link,
        org = encode_safe(&lex.org),
        lex_org = encode_safe(&lex.org),
        lex_orgs = encode_safe(&lex.orgs),
        app_js = APP_JS,
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

fn decode_query_for_title(q: &str) -> String {
    // Replace `+` with space and `%xx` with the corresponding byte;
    // best-effort, used only for the `<title>` text. We don't decode
    // for the focus echo — the JS reparses URLSearchParams from
    // window.location.search after load and overrides the SSR echo.
    let bytes = q.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let h1 = hex_nibble(bytes[i + 1]);
                let h2 = hex_nibble(bytes[i + 2]);
                match (h1, h2) {
                    (Some(a), Some(b)) => {
                        out.push((a << 4) | b);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_carries_design_tokens() {
        let html = render_page("vendor=notional", &SiblingOrigins::default(), &Lexicon::default());
        // Sanity check that the inlined tokens actually made it in —
        // every page should carry the design-system CSS variables
        // since the layout depends on them.
        assert!(html.contains("--bg-base") || html.contains("--accent"));
    }

    #[test]
    fn page_carries_event_table_skeleton() {
        let html = render_page("vendor=notional", &SiblingOrigins::default(), &Lexicon::default());
        assert!(html.contains("events-tbody"));
        assert!(html.contains("sparkline"));
        assert!(html.contains("status-pill"));
    }

    #[test]
    fn page_echoes_query_in_focus_and_title() {
        let html = render_page("vendor=vendor-b&role=sensor", &SiblingOrigins::default(), &Lexicon::default());
        assert!(html.contains("vendor=vendor-b"));
        assert!(html.contains("role=sensor"));
    }

    #[test]
    fn page_escapes_query_html() {
        let html = render_page("vendor=foo<script>alert(1)</script>", &SiblingOrigins::default(), &Lexicon::default());
        assert!(!html.contains("<script>alert(1)</script>"));
        // The escaped form survives.
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn page_includes_panoscope_deeplink_when_origin_given() {
        let html = render_page(
            "vendor=notional",
            &SiblingOrigins {
                panoscope: "https://panoscope.example".into(),
                stethoscope: String::new(),
            },
            &Lexicon::default(),
        );
        assert!(
            html.contains("panoscope.example"),
            "page is missing panoscope deeplink host. html follows:\n{html}"
        );
        // Label is the surface name — same shortening as the
        // stethoscope-link rename.
        assert!(html.contains("panoscope ↗"));
        assert!(!html.contains("open in panoscope"));
    }

    #[test]
    fn page_omits_panoscope_deeplink_when_origin_empty() {
        let html = render_page("vendor=notional", &SiblingOrigins::default(), &Lexicon::default());
        // Neither the rendered label nor the legacy verb form.
        assert!(!html.contains(">panoscope ↗<"));
        assert!(!html.contains("open in panoscope"));
    }

    #[test]
    fn page_includes_app_js_inline() {
        let html = render_page("", &SiblingOrigins::default(), &Lexicon::default());
        // Sentinel from the JS — present iff include_str! resolved.
        assert!(html.contains("EventSource"));
    }

    #[test]
    fn page_includes_speculation_rules() {
        let html = render_page("vendor=notional", &SiblingOrigins::default(), &Lexicon::default());
        assert!(html.contains("speculationrules"));
        assert!(html.contains("\"prerender\""));
    }

    #[test]
    fn empty_query_says_no_slice_in_title() {
        let html = render_page("", &SiblingOrigins::default(), &Lexicon::default());
        assert!(html.contains("flashlight · no slice"));
    }

    // -- Block 94 cross-surface deep-link contract ----------------

    #[test]
    fn stethoscope_link_renders_when_origin_given() {
        let html = render_page(
            "vendor=notional",
            &SiblingOrigins {
                panoscope: String::new(),
                stethoscope: "https://stethoscope.example".into(),
            },
            &Lexicon::default(),
        );
        assert!(html.contains("stethoscope.example"));
        // Label is the surface name (`stethoscope ↗`) per operator
        // feedback; the question form ("is this healthy?")
        // misread as a diagnostic assertion when the hop is just to
        // a sibling tool. Hover-title carries the longer explainer.
        assert!(html.contains("stethoscope ↗"));
        assert!(!html.contains("is this healthy?"));
    }

    #[test]
    fn stethoscope_link_omitted_when_origin_empty() {
        let html = render_page("vendor=notional", &SiblingOrigins::default(), &Lexicon::default());
        // Neither the rendered label nor the legacy question form.
        assert!(!html.contains(">stethoscope ↗<"));
        assert!(!html.contains("is this healthy?"));
    }

    #[test]
    fn sibling_links_preserve_focus_query_string() {
        // Both siblings receive the focus QS verbatim so the operator
        // lands on the same target. flashlight-only params (`role=`,
        // `site=`, `format=`) are silently ignored by stethoscope per
        // the contract's "unknown params silently dropped" rule, but
        // get carried for panoscope which understands `vendor=`/`key=`.
        let html = render_page(
            "vendor=notional&role=sensor",
            &SiblingOrigins {
                panoscope: "https://panoscope.example".into(),
                stethoscope: "https://stethoscope.example".into(),
            },
            &Lexicon::default(),
        );
        // Each sibling href carries the focus QS verbatim — the
        // contract requires that the operator's focus survives the
        // hop. Parse the href out of the rendered anchor and assert
        // the QS substring (decoded form is implementation-specific;
        // every plausible encoding still contains the literal `vendor=
        // notional` and `role=sensor` segments).
        for class in &["panoscope-link", "stethoscope-link"] {
            let a_start = html
                .find(&format!("class=\"{class}"))
                .unwrap_or_else(|| panic!("expected <a> for {class}; html:\n{html}"));
            let href_start = html[a_start..].find("href=\"").expect("href") + a_start + 6;
            let href_end = html[href_start..].find('"').expect("href close");
            let href = &html[href_start..href_start + href_end];
            // Sibling host present.
            assert!(href.contains("example"), "{class} href missing host: {href}");
            // Both focus params survive any URL/HTML encoding.
            assert!(
                href.contains("vendor=notional"),
                "{class} href missing vendor= QS: {href}"
            );
            assert!(
                href.contains("role=sensor"),
                "{class} href missing role= QS: {href}"
            );
        }
    }

    #[test]
    fn sibling_links_have_no_qs_suffix_for_empty_focus() {
        let html = render_page(
            "",
            &SiblingOrigins {
                panoscope: "https://panoscope.example".into(),
                stethoscope: "https://stethoscope.example".into(),
            },
            &Lexicon::default(),
        );
        // Find the panoscope-link <a> and inspect its href attr.
        let a_start = html.find("class=\"panoscope-link").expect("panoscope <a>");
        let href_start = html[a_start..].find("href=\"").expect("href") + a_start + 6;
        let href_end = html[href_start..].find('"').expect("href close");
        let href = &html[href_start..href_start + href_end];
        // No focus → no query string suffix.
        assert!(
            !href.contains('?'),
            "panoscope sibling href unexpectedly carries a query string: {href}"
        );
        // Host survives any escaping.
        assert!(href.contains("panoscope.example"));
    }

    #[test]
    fn truncate_short_strings_unchanged() {
        assert_eq!(truncate("short", 10), "short");
    }

    #[test]
    fn truncate_long_strings_get_ellipsis() {
        let out = truncate("0123456789ABCDEF", 8);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 8);
    }

    #[test]
    fn decode_query_handles_percent_encoding() {
        assert_eq!(decode_query_for_title("vendor=foo%2Fbar"), "vendor=foo/bar");
        assert_eq!(decode_query_for_title("a+b"), "a b");
    }
}
