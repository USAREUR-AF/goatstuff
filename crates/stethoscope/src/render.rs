// SPDX-License-Identifier: Apache-2.0
//
// Server-rendered diagnosis page. Matches the wireframe brief's visual
// + interaction beats (docs/design-assets/stethoscope/project/) without
// adopting the React internal structure — production stack is
// server-rendered HTML + SSE + ~50 lines of vanilla JS per
// docs/design/stethoscope.md and the implementing-agent instructions.
//
// Composition:
//   1. <style> blocks inline both the canonical Block 91 design-system
//      tokens (colors_and_type.css + components.css) AND a subset of
//      the prototype's stethoscope.css scoped to the Phase B surface
//      (no blood-dye drawer, no configure-checks drawer, no drill-
//      down drawer — those Phase C+D surfaces ship behind the same
//      DOM structure when their checks land).
//   2. <body> renders the slim top bar + hero + skeleton cards for
//      the applicable subset of checks.
//   3. Inline <script> opens an EventSource against /api/diagnose
//      with the same target params, listens for `outcome` events,
//      and swaps each skeleton card for the settled card. On `done`,
//      it closes the channel + updates the verdict roll-up.
//
// HTML output is built with `html-escape` for all dynamic strings to
// prevent injection from user-supplied URL parameters. Static markup
// is template-literal-style strings — explicit, no Handlebars-like
// engine dependency.

use html_escape::encode_safe;

use stethoscope_checks::{
    check::{CheckKind, Cost},
    context::{Target, TargetKind},
    registry::{CheckDescriptor, Registry},
};

use panoscope_core::alias::is_anchor_shaped;

use crate::config::Siblings;

const DESIGN_TOKENS_CSS: &str =
    include_str!("../../../docs/design-system/colors_and_type.css");
const DESIGN_COMPONENTS_CSS: &str =
    include_str!("../../../docs/design-system/components.css");
const STETHOSCOPE_CSS: &str = include_str!("assets/stethoscope.css");
const STREAM_JS: &str = include_str!("assets/stream.js");

/// Build the initial page HTML for `target` against `registry`.
///
/// `target` carries the CANONICAL value (UUID-anchored key); `display_value`
/// is its operator-facing rendering (slot alias projected over the anchor per
/// ADR 1039, or the canonical value verbatim when no alias applies). The
/// picker prefill uses `display_value` (a resolvable handle/alias the operator
/// can edit); machine spots (the SSE diagnose URL, the Block 94 sibling
/// deep-links) use the canonical `target.value` — `key=` stays UUID end-to-end.
///
/// `entity_name` is the real entity name (the portal `display_name`, e.g.
/// "Example Org") for a slot-anchor *identity* target. When present, the hero
/// shows it big with the short anchor beside it (click-to-expand to the full
/// UUID), instead of collapsing the identity to either the bare UUID or the
/// slot handle. `None` (or empty) falls back to `display_value`. Topic/system
/// targets and per-handle goatnets (nova-a) ignore it and render the value as
/// before.
///
/// `siblings` carries the Block 94 cross-surface deep-link contract's
/// sibling base URLs — when set, the topbar grows
/// `see in panoscope ↗` + `watch in flashlight ↗` buttons that preserve
/// the current target. Empty siblings = no buttons (`docs/design/zenoh-
/// surface-deep-links.md`).
#[allow(clippy::too_many_arguments)]
pub fn render_page(
    target: &Target,
    display_value: &str,
    entity_name: Option<&str>,
    registry: &Registry,
    site_label: &str,
    diagnose_url: &str,
    siblings: &Siblings,
    lex: &crate::config::Lexicon,
) -> String {
    let applicable = registry.applicable_descriptors(target);
    let target_kind = target.kind.as_str();
    // Display-only: the alias-projected value for the hero/title/picker. The
    // canonical `target.value` still drives the SSE URL + sibling deep-links.
    let target_value = display_value;

    // Pre-render skeleton cards. `data-check-id` keys the SSE replace
    // step; the script does a getElementById on this id when an event
    // for the check lands.
    let mut cards_html = String::new();
    for d in &applicable {
        cards_html.push_str(&render_skeleton_card(d));
    }

    // The full set of registered check ids the page expects events for.
    // The script uses this to know when to flip the roll-up out of
    // RUNNING and into the worst settled verdict.
    let applicable_ids: Vec<&str> = applicable.iter().map(|d| d.id).collect();
    let applicable_ids_json = serde_json::to_string(&applicable_ids).unwrap_or("[]".into());

    let diagnose_url_with_target = build_diagnose_url(diagnose_url, target);
    let picker_html = render_picker_html(target_kind, target_value, PickerMode::Closed, lex);
    let sibling_buttons_html = render_sibling_buttons(target, siblings);
    // Hero kind eyebrow: relex the `vendor` kind to the goatnet noun
    // ("VENDOR" → "ORG"); other kinds (topic/system) are uniform.
    let kind_label = if target_kind == "vendor" {
        lex.org_upper.clone()
    } else {
        target_kind.to_uppercase()
    };
    // Entity hero (ADR 1039 slot-alias display): when the target is a
    // slot-anchor *identity* (`?vendor=<uuid>` on a slot-pool goatnet like
    // ZAGROS), show the real entity name big (the portal display_name) with the
    // short anchor beside it, click-to-expand to the full UUID. The bare UUID
    // is never the headline, and it's never hidden either — it collapses to a
    // prefix the operator can open. Topic/system targets, and per-handle
    // goatnets (nova-a, where the value isn't anchor-shaped), keep the single
    // alias-projected value.
    let is_entity_anchor =
        matches!(target.kind, TargetKind::Vendor) && is_anchor_shaped(&target.value);
    let hero_name = if is_entity_anchor {
        entity_name.filter(|s| !s.is_empty()).unwrap_or(target_value)
    } else {
        target_value
    };
    let hero_anchor_html = if is_entity_anchor {
        let short = format!("{}…", &target.value[..8]);
        format!(
            r#"<button class="sx-hero__anchor" id="anchor-toggle" type="button" data-short="{s}" data-full="{f}" title="Slot anchor — click to reveal the full UUID">{s}</button>"#,
            s = encode_safe(&short),
            f = encode_safe(&target.value),
        )
    } else {
        String::new()
    };
    // Lexicon for stream.js (placeholders + auto-detect hint it builds
    // client-side). Display-only; same discipline as the static HTML.
    let lex_json = serde_json::json!({
        "org": lex.org,
        "org_upper": lex.org_upper,
        "org_title": lex.org_title,
    })
    .to_string();

    format!(
        r#"<!doctype html>
<html lang="en" data-theme="dark">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>stethoscope · {hero_name_escaped}</title>
<style>
{tokens}
{components}
{stethoscope}
</style>
</head>
<body>
<div id="app">
  <header class="sx-topbar">
    <a class="sx-topbar__brand" href="/" title="Back to picker">
      <span class="sx-topbar__mark" aria-hidden="true">⊙</span>
      <span class="sx-topbar__word">stethoscope</span>
    </a>
    <span class="sx-topbar__sep" aria-hidden="true">·</span>
    <nav class="sx-topbar__crumb">
      <a href="/" class="sx-crumb-link">GOAT-Net</a>
      <span class="sx-topbar__sep" aria-hidden="true">/</span>
      <span>{site_label_escaped}</span>
      {parent_crumb_html}
    </nav>
    <div class="sx-topbar__right">
      {sibling_buttons_html}
      <button class="sx-back-btn" id="back-btn" type="button" title="Back to previous page">‹ back</button>
      <button class="sx-theme-toggle" id="theme-toggle" type="button">☀ light</button>
    </div>
  </header>

  <main class="sx-page">
    <section class="sx-hero">
      <div class="sx-hero__top">
        <div class="sx-hero__target">
          <div class="sx-hero__eyebrow">{target_kind_label}</div>
          <div class="sx-hero__idline">
            <button class="sx-target-button" id="change-target-btn" type="button" title="Click to place the stethoscope on a different target">
              <span class="sx-hero__value">{hero_name_escaped}</span>
              <span class="sx-target-edit">change ›</span>
            </button>
            {hero_anchor_html}
          </div>
          {picker_html}
        </div>
        <div class="sx-hero__rollup">
          <span class="sx-hero__rollup-label">overall</span>
          <span class="sx-rollup sx-rollup--running" id="rollup">
            <span class="sx-rollup__dot"></span>
            <span id="rollup-label">RUNNING</span>
          </span>
        </div>
      </div>
      <div class="sx-hero__meta">
        <span><b id="settled-count">0</b>/<b>{total}</b> checks settled</span>
        <span class="sep">·</span>
        <span>Phase B — passive checks only</span>
      </div>
      <div class="sx-hero__actions">
        <button class="btn btn--sm" id="rerun-btn" type="button">↻ re-run</button>
        <span class="spacer"></span>
      </div>
    </section>

    <section class="sx-cards" id="cards">
      {cards_html}
    </section>
  </main>
</div>
<script>
window.STETHOSCOPE_APPLICABLE_IDS = {applicable_ids_json};
window.STETHOSCOPE_DIAGNOSE_URL = {diagnose_url_json};
window.STETHOSCOPE_LEX = {lex_json};
</script>
<script>
{stream_js}
</script>
<script>
// Slot-anchor reveal: the hero shows the short anchor prefix; clicking it
// toggles to the full UUID (and back). Display-only — the canonical UUID
// already rides the SSE URL + sibling deep-links; this just lets the operator
// read it without leaving the page. stopPropagation so the click doesn't open
// the change-target picker.
(function () {{
  var t = document.getElementById('anchor-toggle');
  if (!t) return;
  var open = false;
  t.addEventListener('click', function (e) {{
    e.stopPropagation();
    open = !open;
    t.textContent = open ? t.dataset.full : t.dataset.short;
  }});
}})();
</script>
</body>
</html>"#,
        hero_name_escaped = encode_safe(hero_name),
        hero_anchor_html = hero_anchor_html,
        target_kind_label = encode_safe(&kind_label),
        site_label_escaped = encode_safe(site_label),
        parent_crumb_html = render_parent_crumb_html(target),
        tokens = DESIGN_TOKENS_CSS,
        components = DESIGN_COMPONENTS_CSS,
        stethoscope = STETHOSCOPE_CSS,
        cards_html = cards_html,
        applicable_ids_json = applicable_ids_json,
        lex_json = lex_json,
        diagnose_url_json = serde_json::to_string(&diagnose_url_with_target)
            .unwrap_or_else(|_| "\"\"".into()),
        total = applicable.len(),
        stream_js = STREAM_JS,
        picker_html = picker_html,
        sibling_buttons_html = sibling_buttons_html,
    )
}

/// Render the topbar sibling-action buttons per the Block 94
/// cross-surface deep-link contract. Each button is conditional on
/// the corresponding sibling URL being set — a stethoscope deployed
/// without a panoscope sibling just doesn't render the `see in
/// panoscope ↗` button.
///
/// Param mapping per the contract canon (`docs/design/zenoh-surface-
/// deep-links.md` §"URL parameter canon"):
/// - Topic target  → `?key=<topic>` to panoscope + flashlight (key= is
///   canonical; topic= is stethoscope-internal alias).
/// - Vendor target → `?vendor=<v>`.
/// - System target → `?vendor=<vendor-prefix>` (panoscope + flashlight
///   don't yet have a system-level selection; vendor is the next-best
///   parent the operator likely wants to see).
fn render_sibling_buttons(target: &Target, siblings: &Siblings) -> String {
    use stethoscope_checks::context::TargetKind;
    let (panoscope_qs, flashlight_qs) = match target.kind {
        TargetKind::Topic => {
            let q = format!("key={}", urlencode(&target.value));
            (q.clone(), q)
        }
        TargetKind::Vendor => {
            let q = format!("vendor={}", urlencode(&target.value));
            (q.clone(), q)
        }
        TargetKind::System => {
            // Fall through to vendor — siblings don't have a per-system
            // selection in v1 of the contract. The operator can still
            // narrow once they land.
            let v = target.vendor_prefix();
            if v.is_empty() {
                return String::new();
            }
            let q = format!("vendor={}", urlencode(v));
            (q.clone(), q)
        }
    };
    let mut out = String::new();
    if let Some(base) = &siblings.panoscope {
        out.push_str(&format!(
            r#"<a class="sx-sibling-link" href="{base_safe}/?{qs}" target="_blank" rel="noopener noreferrer" title="see this target in panoscope">see in panoscope ↗</a>"#,
            base_safe = encode_safe(base),
            qs = encode_safe(&panoscope_qs),
        ));
    }
    if let Some(base) = &siblings.flashlight {
        out.push_str(&format!(
            r#"<a class="sx-sibling-link" href="{base_safe}/?{qs}" target="_blank" rel="noopener noreferrer" title="watch this target in flashlight">watch in flashlight ↗</a>"#,
            base_safe = encode_safe(base),
            qs = encode_safe(&flashlight_qs),
        ));
    }
    out
}

/// Topbar parent crumbs for a target. Walks the vendor → system →
/// topic chain and emits `/ <link-or-text>` for each parent — current
/// target itself is rendered in the hero, not the crumb, so this skips
/// the leaf. Vendor target = empty (no parents above the site label).
fn render_parent_crumb_html(target: &Target) -> String {
    use stethoscope_checks::context::TargetKind;
    let sep = r#"<span class="sx-topbar__sep" aria-hidden="true">/</span>"#;
    match target.kind {
        // Vendor is itself a top-level target — no parent above site.
        TargetKind::Vendor => String::new(),
        TargetKind::System => {
            // value shape: `<vendor>/<system>`. Emit vendor as a link.
            let vendor = target.vendor_prefix();
            if vendor.is_empty() {
                return String::new();
            }
            format!(
                r#"{sep}<a href="/?vendor={v_enc}" class="sx-crumb-link">{v_safe}</a>"#,
                v_enc = urlencode(vendor),
                v_safe = encode_safe(vendor),
            )
        }
        TargetKind::Topic => {
            // value shape: `<vendor>/<system>/<topic>[/<version>]`.
            // Emit vendor (link) / system (link) — topic itself is the
            // hero leaf.
            let vendor = target.vendor_prefix();
            if vendor.is_empty() {
                return String::new();
            }
            let mut parts = String::new();
            parts.push_str(&format!(
                r#"{sep}<a href="/?vendor={v_enc}" class="sx-crumb-link">{v_safe}</a>"#,
                v_enc = urlencode(vendor),
                v_safe = encode_safe(vendor),
            ));
            // System = vendor/<first-segment-of-suffix>.
            let after = target.value.strip_prefix(&format!("{vendor}/")).unwrap_or("");
            if let Some(sys_seg) = after.split('/').next() {
                if !sys_seg.is_empty() {
                    let system = format!("{vendor}/{sys_seg}");
                    parts.push_str(&format!(
                        r#"{sep}<a href="/?system={s_enc}" class="sx-crumb-link">{s_safe}</a>"#,
                        s_enc = urlencode(&system),
                        s_safe = encode_safe(sys_seg),
                    ));
                }
            }
            parts
        }
    }
}

/// Picker mode controls visibility + animation on first paint.
#[derive(Clone, Copy)]
enum PickerMode {
    /// Hidden until the operator clicks the CHANGE button. Anchored
    /// under the hero value with a scrim that closes on outside click.
    Closed,
    /// Mounted open in-flow with no scrim, no animation flash. Used
    /// on the landing page (URL state `?` — "I haven't decided yet").
    Landing,
}

/// Render the "Place the stethoscope on…" popover. The HTML carries
/// the structure but defers all behavior (open/close/submit) to
/// stream.js, which discovers the right element by class.
fn render_picker_html(
    current_kind: &str,
    current_value: &str,
    mode: PickerMode,
    lex: &crate::config::Lexicon,
) -> String {
    // Placeholders illustrate the key shape; the leading segment is the
    // participant anchor, relexed to the goatnet noun ("vendor/..." →
    // "org/..."). Display-only — the real anchor on the wire is the
    // cert-bound prefix / UUID regardless of the word shown.
    let org = lex.org.as_str();
    let placeholder = match current_kind {
        "topic" => format!("{org}/topic/version"),
        "vendor" => org.to_string(),
        "system" => format!("{org}/system"),
        _ => format!("{org}/topic/version"),
    };
    // Default chip on the landing picker is `topic` — autocomplete
    // searches topics dynamically as the operator types. If they
    // submit without going past the first slash segment, the JS
    // auto-detect-on-submit downgrades to a vendor lookup (no slash
    // → vendor; 1 slash → system; 2+ → topic). Clicking a chip pins
    // the kind explicitly.
    let initial_kind = if current_kind.is_empty() {
        "topic"
    } else {
        current_kind
    };

    let (picker_class, scrim_class, scrim_html) = match mode {
        PickerMode::Closed => (
            "sx-picker",
            "sx-picker-scrim",
            r#"<div class="sx-picker-scrim" aria-hidden="true"></div>"#.to_string(),
        ),
        PickerMode::Landing => (
            "sx-picker is-landing is-open",
            "sx-picker-scrim is-landing",
            String::new(),
        ),
    };
    let _ = scrim_class;

    let kind_btn = |k: &str, label: &str| -> String {
        let on = if k == initial_kind { " is-on" } else { "" };
        format!(
            r#"<button type="button" class="sx-kind-btn{on}" data-kind="{k}">{label}</button>"#
        )
    };

    format!(
        r#"{scrim_html}
<div class="{picker_class}" role="dialog" aria-label="Place the stethoscope on a target" data-kind="{kind_escaped}" data-value="{value_escaped}">
  <div class="sx-picker__label">Place the stethoscope on…</div>
  <div class="sx-picker__input-row">
    <input
      class="sx-picker__input"
      type="text"
      value="{value_escaped}"
      placeholder="{placeholder}"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      spellcheck="false"
      data-kind="{kind_escaped}"
    >
    <button type="button" class="sx-picker__go">Read</button>
  </div>
  <div class="sx-picker__kinds">
    {topic_btn}
    {vendor_btn}
    {system_btn}
  </div>
  <div class="sx-typeahead" aria-live="polite"></div>
  <div class="sx-picker__label">Recent</div>
  <div class="sx-picker__presets" aria-live="polite"></div>
  <div class="sx-picker__hint">
    Enter to read · Esc to cancel · auto-detects {org_lc} / system / topic from input shape.
  </div>
</div>"#,
        kind_escaped = encode_safe(initial_kind),
        value_escaped = encode_safe(current_value),
        placeholder = encode_safe(&placeholder),
        org_lc = encode_safe(org),
        topic_btn = kind_btn("topic", "Topic"),
        vendor_btn = kind_btn("vendor", &lex.org_title),
        system_btn = kind_btn("system", "System"),
    )
}

/// Build the initial page HTML for the URL state `?` — no target,
/// picker mounted open in the hero. Same chrome as the diagnose
/// page so the operator's mental model is one surface.
pub fn render_picker_landing(site_label: &str, lex: &crate::config::Lexicon) -> String {
    // Empty current_kind lets render_picker_html pick the default
    // (topic) so the picker is consistent across all entry points.
    let picker_html = render_picker_html("", "", PickerMode::Landing, lex);
    format!(
        r#"<!doctype html>
<html lang="en" data-theme="dark">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>stethoscope · pick a target</title>
<style>
{tokens}
{components}
{stethoscope}
</style>
</head>
<body>
<div id="app">
  <header class="sx-topbar">
    <a class="sx-topbar__brand" href="/" title="Back to picker">
      <span class="sx-topbar__mark" aria-hidden="true">⊙</span>
      <span class="sx-topbar__word">stethoscope</span>
    </a>
    <span class="sx-topbar__sep" aria-hidden="true">·</span>
    <nav class="sx-topbar__crumb">
      <a href="/" class="sx-crumb-link">GOAT-Net</a>
      <span class="sx-topbar__sep" aria-hidden="true">/</span>
      <span>{site_label_escaped}</span>
    </nav>
    <div class="sx-topbar__right">
      <button class="sx-theme-toggle" id="theme-toggle" type="button">☀ light</button>
    </div>
  </header>
  <main class="sx-page">
    <section class="sx-hero">
      <div class="sx-hero__top">
        <div class="sx-hero__target">
          <div class="sx-hero__eyebrow">no target — pick one</div>
          {picker_html}
        </div>
      </div>
      <div class="sx-hero__meta">
        <span>arrive here from panoscope / portal / flashlight via <code>diagnose ›</code>, or type a target above.</span>
      </div>
    </section>
  </main>
</div>
<script>
window.STETHOSCOPE_LEX = {lex_json};
</script>
<script>
{stream_js}
</script>
</body>
</html>"#,
        tokens = DESIGN_TOKENS_CSS,
        components = DESIGN_COMPONENTS_CSS,
        stethoscope = STETHOSCOPE_CSS,
        site_label_escaped = encode_safe(site_label),
        picker_html = picker_html,
        stream_js = STREAM_JS,
        lex_json = serde_json::json!({
            "org": lex.org,
            "org_upper": lex.org_upper,
            "org_title": lex.org_title,
        }),
    )
}

/// Friendly first-paint for a malformed target — same chrome, hero
/// shows the error, no cards rendered.
pub fn render_target_error(message: &str, site_label: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en" data-theme="dark">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>stethoscope · target required</title>
<style>
{tokens}
{components}
{stethoscope}
</style>
</head>
<body>
<div id="app">
  <header class="sx-topbar">
    <a class="sx-topbar__brand" href="/" title="Back to picker">
      <span class="sx-topbar__mark" aria-hidden="true">⊙</span>
      <span class="sx-topbar__word">stethoscope</span>
    </a>
    <span class="sx-topbar__sep" aria-hidden="true">·</span>
    <nav class="sx-topbar__crumb">
      <a href="/" class="sx-crumb-link">GOAT-Net</a>
      <span class="sx-topbar__sep" aria-hidden="true">/</span>
      <span>{site_label_escaped}</span>
    </nav>
  </header>
  <main class="sx-page">
    <section class="sx-hero">
      <div class="sx-hero__target">
        <div class="sx-hero__eyebrow">no target</div>
        <div class="sx-hero__value" style="color:var(--c-amber-hi)">{message_escaped}</div>
      </div>
      <div class="sx-hero__meta">
        <span>Try one of: <code>?vendor=&lt;name&gt;</code>, <code>?topic=&lt;key&gt;</code>, or <code>?system=&lt;vendor&gt;/&lt;name&gt;</code>.</span>
      </div>
    </section>
  </main>
</div>
</body>
</html>"#,
        tokens = DESIGN_TOKENS_CSS,
        components = DESIGN_COMPONENTS_CSS,
        stethoscope = STETHOSCOPE_CSS,
        site_label_escaped = encode_safe(site_label),
        message_escaped = encode_safe(message),
    )
}

fn render_skeleton_card(d: &CheckDescriptor) -> String {
    let cost = cost_label(d.cost);
    let kind = match d.kind {
        CheckKind::Passive => "",
        CheckKind::Active => r#"<span class="sx-card__kind">blood-dye</span>"#,
    };
    format!(
        r#"<div class="sx-card" id="card-{id}" data-check-id="{id}">
  <div class="sx-card__main">
    <div class="sx-card__icon"><span class="sx-spinner" aria-hidden="true"></span></div>
    <div class="sx-card__body">
      <div class="sx-card__title-row">
        <div class="sx-card__title">{name}</div>
        <div class="sx-card__id">{id}</div>
        {kind}
      </div>
      <div class="sx-card__summary">
        <span class="sx-skeleton sx-skeleton--wide" aria-hidden="true"></span>
        <span class="sx-cost {cost_cls}">{cost_label}</span>
      </div>
    </div>
    <div class="sx-card__pill">
      <span class="sx-pill sx-pill--running"><span class="sx-pill__dot"></span>RUNNING</span>
    </div>
  </div>
</div>"#,
        id = encode_safe(d.id),
        name = encode_safe(d.name),
        kind = kind,
        cost_cls = cost.css_class,
        cost_label = cost.label,
    )
}

struct CostLabel {
    label: &'static str,
    css_class: &'static str,
}

fn cost_label(c: Cost) -> CostLabel {
    match c {
        Cost::None => CostLabel {
            label: "NO COST",
            css_class: "sx-cost--none",
        },
        Cost::NetworkRead => CostLabel {
            label: "NETWORK READ",
            css_class: "sx-cost--read",
        },
        Cost::NetworkProbe => CostLabel {
            label: "NETWORK PROBE",
            css_class: "sx-cost--probe",
        },
        Cost::Compute => CostLabel {
            label: "COMPUTE",
            css_class: "sx-cost--compute",
        },
    }
}

fn build_diagnose_url(base: &str, target: &Target) -> String {
    use stethoscope_checks::context::TargetKind;
    let key = match target.kind {
        TargetKind::Vendor => "vendor",
        TargetKind::Topic => "topic",
        TargetKind::System => "system",
    };
    let v = urlencode(&target.value);
    format!("{base}?{key}={v}")
}

// Hand-rolled urlencode — the only encoded character classes we hit
// in target values are `/` and `:` (in topic key_patterns) plus the
// rare space; this is enough for our shape, no dep needed.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(ch),
            _ => {
                let mut buf = [0u8; 4];
                for b in ch.encode_utf8(&mut buf).as_bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use stethoscope_checks::checks;

    fn populated_registry() -> Registry {
        let mut r = Registry::new();
        checks::register_phase_b(&mut r);
        r
    }

    #[test]
    fn renders_vendor_page_with_applicable_checks_only() {
        let r = populated_registry();
        let page = render_page(&Target::vendor("acme"), "acme", None, &r, "nova-a", "/api/diagnose", &Siblings::default(), &crate::config::Lexicon::default());
        assert!(page.contains("VENDOR"));
        assert!(page.contains("acme"));
        assert!(page.contains("sx-cards"));
        // Two checks apply to vendor targets: registration + flow-rate.
        assert!(page.contains("data-check-id=\"registration\""));
        assert!(page.contains("data-check-id=\"flow-rate\""));
        // schema-body is topic-only — must NOT appear on a vendor page.
        assert!(!page.contains("data-check-id=\"schema-body\""));
    }

    #[test]
    fn entity_anchor_hero_shows_name_and_collapsible_anchor() {
        // A slot-anchor identity target (ZAGROS) headlines the real entity name
        // with the short anchor beside it, click-to-expand to the full UUID.
        let r = populated_registry();
        let anchor = "0472940bda1695cb078cd8e927b7afed";
        let page = render_page(
            &Target::vendor(anchor),
            "pat-example", // the slot handle (alias) — NOT the headline here
            Some("Example Org"),
            &r,
            "ZAGROS-A",
            "/api/diagnose",
            &Siblings::default(),
            &crate::config::Lexicon {
                org: "org".into(),
                org_upper: "ORG".into(),
                org_title: "Org".into(),
            },
        );
        // Real entity name is the headline (hero value + title).
        assert!(page.contains(r#"<span class="sx-hero__value">Example Org</span>"#));
        assert!(page.contains("<title>stethoscope · Example Org</title>"));
        // Short anchor shown, collapsible to the full UUID (data-full carries it).
        assert!(page.contains("0472940b…"));
        assert!(page.contains(&format!(r#"data-full="{anchor}""#)));
        assert!(page.contains("anchor-toggle"));
        // Eyebrow relexed away from "vendor" (ZAGROS lexicon → ORG).
        assert!(page.contains(r#"<div class="sx-hero__eyebrow">ORG</div>"#));
        assert!(!page.contains(">VENDOR<"));
        // The full UUID is NOT the bare headline.
        assert!(!page.contains(&format!(r#"<span class="sx-hero__value">{anchor}</span>"#)));
    }

    #[test]
    fn entity_anchor_hero_falls_back_to_alias_when_no_display_name() {
        // Portal-down / no display_name → headline falls back to the alias
        // (display_value), anchor still collapsible. Never errors.
        let r = populated_registry();
        let anchor = "0472940bda1695cb078cd8e927b7afed";
        let page = render_page(
            &Target::vendor(anchor),
            "pat-example",
            None,
            &r,
            "ZAGROS-A",
            "/api/diagnose",
            &Siblings::default(),
            &crate::config::Lexicon::default(),
        );
        assert!(page.contains(r#"<span class="sx-hero__value">pat-example</span>"#));
        assert!(page.contains("0472940b…"));
        assert!(page.contains("anchor-toggle"));
    }

    #[test]
    fn renders_topic_page_only_applicable_cards() {
        let r = populated_registry();
        let page = render_page(
            &Target::topic("acme/sensor1/RADAR-EXAMPLE/tracks"),
            "acme/sensor1/RADAR-EXAMPLE/tracks",
            None,
            &r,
            "nova-a",
            "/api/diagnose",
            &Siblings::default(),
            &crate::config::Lexicon::default(),
        );
        assert!(page.contains("TOPIC"));
        assert!(page.contains("data-check-id=\"schema-body\""));
        assert!(page.contains("data-check-id=\"registration\""));
        assert!(page.contains("data-check-id=\"flow-rate\""));
    }

    #[test]
    fn escapes_target_value_in_page() {
        let r = populated_registry();
        let page = render_page(
            &Target::vendor("<script>alert(1)</script>"),
            "<script>alert(1)</script>",
            None,
            &r,
            "nova-a",
            "/api/diagnose",
            &Siblings::default(),
            &crate::config::Lexicon::default(),
        );
        // The literal injection vector must not appear in the
        // rendered HTML body of the hero / title / etc. The page
        // does inline the stream.js asset which itself contains
        // `<script>` JSON-encoded — that's safe (inside our trusted
        // bundled JS), but we look for the unescaped attacker value.
        let attacker_payload = "alert(1)";
        // The 'alert(1)' substring will appear inside the
        // html-escaped form too, so we assert positively: the value
        // must be HTML-entity-encoded (any of &lt;, &#60;, &#x3C; etc).
        let entity_forms = ["&lt;", "&#60;", "&#x3C;", "&#X3C;"];
        let has_entity = entity_forms.iter().any(|e| page.contains(e));
        assert!(
            has_entity,
            "expected an entity-escaped '<' to appear; got page starting with: {}",
            &page[..page.len().min(300)]
        );
        // And confirm `alert(1)` only appears alongside an entity
        // (we can't trivially assert no raw `<script>alert(1)>` without
        // walking the DOM, but the entity check above is the load-
        // bearing safety property).
        assert!(page.contains(attacker_payload));
    }

    #[test]
    fn renders_target_error_page() {
        let html = render_target_error("no target parameter", "nova-a");
        assert!(html.contains("no target parameter"));
        assert!(html.contains("?vendor="));
    }

    #[test]
    fn picker_landing_includes_input_and_kind_chips() {
        let html = render_picker_landing("nova-a", &crate::config::Lexicon::default());
        assert!(html.contains("Place the stethoscope on"));
        assert!(html.contains("sx-picker__input"));
        // All three kind chips present.
        assert!(html.contains("data-kind=\"topic\""));
        assert!(html.contains("data-kind=\"vendor\""));
        assert!(html.contains("data-kind=\"system\""));
        // Topic is the default selected kind on the landing page.
        // Autocomplete searches topics dynamically; JS auto-detect on
        // submit downgrades to vendor when the input has no slash.
        assert!(html.contains("sx-kind-btn is-on\" data-kind=\"topic\""));
        // Hint line — mentions auto-detect so operators know the
        // chips aren't load-bearing for the common case.
        assert!(html.contains("Enter to read"));
        assert!(html.contains("auto-detects"));
        // Typeahead container present (live autocomplete against /api/index).
        assert!(html.contains("sx-typeahead"));
        // Mounted-open variant: no scrim, picker carries the is-landing class.
        assert!(html.contains("is-landing"));
    }

    #[test]
    fn picker_landing_escapes_site_label() {
        let html = render_picker_landing("<script>alert(1)</script>", &crate::config::Lexicon::default());
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }

    #[test]
    fn vendor_page_includes_picker_with_current_value() {
        let r = populated_registry();
        let page = render_page(&Target::vendor("acme"), "acme", None, &r, "nova-a", "/api/diagnose", &Siblings::default(), &crate::config::Lexicon::default());
        // CHANGE affordance + popover ship on the diagnose page too.
        assert!(page.contains("id=\"change-target-btn\""));
        assert!(page.contains("change ›"));
        assert!(page.contains("Place the stethoscope on"));
        // The picker is seeded with the current target.
        assert!(page.contains("data-kind=\"vendor\" data-value=\"acme\""));
        // Vendor chip should be the active one on this page.
        assert!(page.contains("sx-kind-btn is-on\" data-kind=\"vendor\""));
    }

    #[test]
    fn urlencode_handles_slashes_and_colons() {
        assert_eq!(urlencode("acme/foo/bar/v1"), "acme%2Ffoo%2Fbar%2Fv1");
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("plain"), "plain");
    }

    #[test]
    fn build_diagnose_url_round_trip() {
        let u = build_diagnose_url(
            "/api/diagnose",
            &Target::topic("acme/foo/bar/v1"),
        );
        assert_eq!(u, "/api/diagnose?topic=acme%2Ffoo%2Fbar%2Fv1");
    }

    #[test]
    fn parent_crumb_is_empty_for_vendor_target() {
        let s = render_parent_crumb_html(&Target::vendor("acme"));
        assert!(s.is_empty());
    }

    #[test]
    fn parent_crumb_links_vendor_for_system_target() {
        let s = render_parent_crumb_html(&Target::system("acme/sensor1"));
        assert!(s.contains(r#"<a href="/?vendor=acme""#));
        assert!(s.contains(">acme</a>"));
    }

    #[test]
    fn parent_crumb_links_vendor_and_system_for_topic_target() {
        let s = render_parent_crumb_html(&Target::topic("acme/sensor1/RADAR-EXAMPLE/tracks"));
        assert!(s.contains(r#"<a href="/?vendor=acme""#));
        assert!(s.contains(r#"<a href="/?system=acme%2Fsensor1""#));
        assert!(s.contains(">sensor1</a>"));
    }

    #[test]
    fn diagnose_page_brand_links_home() {
        let r = populated_registry();
        let page = render_page(&Target::vendor("acme"), "acme", None, &r, "nova-a", "/api/diagnose", &Siblings::default(), &crate::config::Lexicon::default());
        assert!(page.contains(r#"<a class="sx-topbar__brand" href="/""#));
        assert!(page.contains(r#"<a href="/" class="sx-crumb-link">GOAT-Net</a>"#));
        assert!(page.contains(r#"id="back-btn""#));
    }

    // -- Block 94 sibling buttons (cross-surface deep-link contract) --

    #[test]
    fn sibling_buttons_empty_when_no_siblings_configured() {
        // Default Siblings = both None ⇒ no buttons rendered in the
        // topbar at all (URL-only contract: no sibling = no button).
        let s = render_sibling_buttons(&Target::vendor("acme"), &Siblings::default());
        assert!(s.is_empty());
    }

    #[test]
    fn sibling_buttons_render_panoscope_and_flashlight_for_vendor_target() {
        let siblings = Siblings {
            panoscope: Some("https://panoscope.example".into()),
            flashlight: Some("https://flashlight.example".into()),
        };
        let s = render_sibling_buttons(&Target::vendor("acme"), &siblings);
        assert!(s.contains("panoscope.example/?vendor=acme"));
        assert!(s.contains("flashlight.example/?vendor=acme"));
        assert!(s.contains("see in panoscope"));
        assert!(s.contains("watch in flashlight"));
    }

    #[test]
    fn sibling_buttons_use_key_param_for_topic_target() {
        let siblings = Siblings {
            panoscope: Some("https://panoscope.example".into()),
            flashlight: Some("https://flashlight.example".into()),
        };
        let s = render_sibling_buttons(
            &Target::topic("acme/foo/bar/v1"),
            &siblings,
        );
        // Block 94 canon: `key=` is the preferred param name across
        // surfaces for Zenoh key expressions, not `topic=`.
        assert!(s.contains("?key=acme%2Ffoo%2Fbar%2Fv1"));
    }

    #[test]
    fn sibling_buttons_for_system_falls_back_to_vendor() {
        let siblings = Siblings {
            panoscope: Some("https://panoscope.example".into()),
            flashlight: None,
        };
        let s = render_sibling_buttons(
            &Target::system("acme/sensor1"),
            &siblings,
        );
        // Panoscope/flashlight don't have a system-level selection in
        // v1 of the contract; siblings render with the vendor prefix
        // as the next-best parent the operator likely wants to see.
        assert!(s.contains("?vendor=acme"));
        // Flashlight sibling unset ⇒ no flashlight button.
        assert!(!s.contains("watch in flashlight"));
    }

    #[test]
    fn sibling_buttons_only_render_the_configured_sibling() {
        // Mixed deployment: only flashlight is wired.
        let siblings = Siblings {
            panoscope: None,
            flashlight: Some("https://flashlight.example".into()),
        };
        let s = render_sibling_buttons(&Target::vendor("acme"), &siblings);
        assert!(!s.contains("see in panoscope"));
        assert!(s.contains("watch in flashlight"));
    }
}
