//! Server-rendered pages (Phase C). One view-model, two renderers: this HTML
//! and the goat-cli lens both draw from [`crate::view::EntityView`] — they
//! can never disagree. Design-system tokens embedded at compile time
//! (docs/design-system/, ADR 1018 canon), stethoscope pattern.

use crate::event::{Attribution, SourceState};
use crate::view::{CapabilityPane, EntityView};

const DESIGN_TOKENS_CSS: &str = include_str!("../../../docs/design-system/colors_and_type.css");
const DESIGN_COMPONENTS_CSS: &str = include_str!("../../../docs/design-system/components.css");
const MICROSCOPE_CSS: &str = include_str!("assets/microscope.css");

/// Render a capability list, keeping "we asked and the answer was none"
/// distinct from "we never asked". Only the former is a claim about the
/// entity; conflating them is how a read-only lens invents a false negative.
fn capability_value(resolved: bool, values: &[String], unread_reason: &str) -> String {
    if !values.is_empty() {
        return esc(&values.join(", "));
    }
    if resolved {
        "(none)".to_string()
    } else {
        format!(r#"<span class="unknown">unknown — {}</span>"#, esc(unread_reason))
    }
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Typeahead for every search box on the surface.
///
/// WHY IT EXISTS. The box took a selector and nothing else, so examining
/// someone required already knowing their exact email, handle, anchor or peer
/// name — and a near miss returned a page that resolved nothing, which on
/// this surface reads uncomfortably like "no such person". Suggestions turn
/// the box into a picker over the selectors this deployment can actually
/// resolve.
///
/// MATCH ANYWHERE. `?q=` matches at the start, in the middle, or at the end
/// of a selector or label (`/api/suggest`, [`crate::suggest`]) — typing
/// `sample` finds `sam.sample@…`, `gmail.com` finds everyone on that domain.
/// Position only affects ORDER.
///
/// It attaches to whatever search inputs the page has (the top bar's and the
/// landing picker's), builds its list with `textContent` so a label can never
/// become markup, and degrades to the plain form it wrapped: with JS off, or
/// the endpoint refusing, the box still submits and the page still loads.
const SUGGEST_JS: &str = r#"<script>
(function(){
  var boxes=document.querySelectorAll('form.navsearch input[name=id],form.picker input[name=id]');
  Array.prototype.forEach.call(boxes,attach);
  function attach(input){
    input.setAttribute('autocomplete','off');
    input.setAttribute('role','combobox');
    input.setAttribute('aria-autocomplete','list');
    input.setAttribute('aria-expanded','false');
    var wrap=document.createElement('div');wrap.className='sugwrap';
    input.parentNode.insertBefore(wrap,input);wrap.appendChild(input);
    var list=document.createElement('ul');list.className='suggest';list.hidden=true;
    list.setAttribute('role','listbox');wrap.appendChild(list);
    var items=[],sel=-1,timer=null,seq=0;
    function close(){list.hidden=true;list.textContent='';items=[];sel=-1;
      input.setAttribute('aria-expanded','false');}
    function mark(i){
      var kids=list.querySelectorAll('li.sug-item');
      for(var k=0;k<kids.length;k++){kids[k].classList.toggle('on',k===i);
        kids[k].setAttribute('aria-selected',k===i?'true':'false');}
      sel=i;}
    function go(i){if(items[i])location.href='/entity?id='+encodeURIComponent(items[i].selector);}
    /* Show WHERE the match landed — start, middle or end — without ever
       putting server text through innerHTML. */
    function marked(text,q){
      var frag=document.createDocumentFragment();
      var hay=text.toLowerCase(),needle=q.toLowerCase(),at=hay.indexOf(needle),from=0;
      while(at>=0&&needle){
        frag.appendChild(document.createTextNode(text.slice(from,at)));
        var m=document.createElement('mark');m.textContent=text.slice(at,at+needle.length);
        frag.appendChild(m);from=at+needle.length;at=hay.indexOf(needle,from);}
      frag.appendChild(document.createTextNode(text.slice(from)));
      return frag;}
    function note(msg){
      var li=document.createElement('li');li.className='sug-note';li.textContent=msg;
      list.appendChild(li);}
    function render(d){
      list.textContent='';items=[];sel=-1;
      if(d.roster_read===false){
        /* Unread is not empty. Saying "no matches" here would be this surface
           inventing the false negative it exists to prevent. */
        note('suggestions unavailable — the roster could not be read. Type the full selector and press examine.');
      }else if(!d.matches||!d.matches.length){
        note('no match in the roster — press Enter to examine "'+d.q+'" as typed');
      }else{
        items=d.matches;
        items.forEach(function(m,i){
          var li=document.createElement('li');li.className='sug-item';
          li.setAttribute('role','option');li.setAttribute('aria-selected','false');
          var lab=document.createElement('span');lab.className='sug-label';
          lab.appendChild(marked(m.label,d.q));li.appendChild(lab);
          var kind=document.createElement('span');kind.className='sug-kind';
          kind.textContent=m.kind;li.appendChild(kind);
          if(m.detail){var det=document.createElement('span');det.className='sug-detail';
            det.textContent=m.detail;li.appendChild(det);}
          li.addEventListener('mousedown',function(e){e.preventDefault();go(i);});
          li.addEventListener('mouseenter',function(){mark(i);});
          list.appendChild(li);});
      }
      list.hidden=false;input.setAttribute('aria-expanded','true');}
    function ask(){
      var q=input.value.trim();
      if(q.length<2){close();return;}
      var mine=++seq;
      fetch('/api/suggest?limit=10&q='+encodeURIComponent(q),{credentials:'same-origin'})
        .then(function(r){if(!r.ok)throw new Error('HTTP '+r.status);return r.json();})
        .then(function(d){if(mine===seq)render(d);})
        .catch(function(){if(mine===seq)close();});}
    input.addEventListener('input',function(){clearTimeout(timer);timer=setTimeout(ask,160);});
    input.addEventListener('keydown',function(e){
      if(list.hidden||!items.length){
        if(e.key==='ArrowDown'){clearTimeout(timer);ask();}
        return;}
      if(e.key==='ArrowDown'){e.preventDefault();mark((sel+1)%items.length);}
      else if(e.key==='ArrowUp'){e.preventDefault();mark((sel-1+items.length)%items.length);}
      else if(e.key==='Enter'){if(sel>=0){e.preventDefault();go(sel);}}
      else if(e.key==='Escape'){close();}});
    input.addEventListener('blur',function(){setTimeout(close,150);});
  }
})();
</script>"#;

/// Every page gets the same top bar: brand, a live search box, and the
/// cross-links. Search previously existed ONLY on the landing page, so once a
/// reader had examined anyone the sole way to examine someone else was to
/// clear the URL by hand — and the graph's way back was a link at the very
/// bottom of a page that can be thousands of pixels tall. Navigation belongs
/// where the reader already is, not where the document happens to end.
fn shell(site_label: &str, body: &str) -> String {
    shell_with(site_label, body, "")
}

/// `current` pre-fills the search box so a reader can edit the selector they
/// are on rather than retyping it.
pub(crate) fn shell_with(site_label: &str, body: &str, current: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>microscope · {site}</title>
<style>{tokens}
{components}
{own}</style></head>
<body><header class="topbar"><a class="brand" href="/">microscope</a>
<span class="site">{site}</span>
<form class="navsearch" action="/entity" method="get" role="search">
<input name="id" value="{current}" placeholder="email · handle · anchor · peer" aria-label="examine another entity">
<button type="submit">examine</button></form>
<span class="doctrine">read-only · every look is audited</span></header>
<main>{body}</main>{suggest}</body></html>"#,
        site = esc(site_label),
        current = esc(current),
        tokens = DESIGN_TOKENS_CSS,
        components = DESIGN_COMPONENTS_CSS,
        own = MICROSCOPE_CSS,
        body = body,
        suggest = SUGGEST_JS,
    )
}

/// The landing page: the picker form, nothing else.
pub fn home(site_label: &str) -> String {
    shell(
        site_label,
        r#"<section class="hero"><h1>Per-entity behavior lens</h1>
<p>One entity, every layer: what they may do, what they did, and where a
refusal happened. RCA · concern watch · retained-actor watch (ADR 1090).</p>
<form class="picker" action="/entity" method="get">
<input name="id" placeholder="email · handle · anchor UUID · slot alias" autofocus>
<button type="submit">examine</button></form>
<p class="pickerhint">Type any fragment — start, middle or end. Suggestions come from
the portal roster, slot aliases and mesh peers; a selector this deployment has
never heard of is still examined as typed.</p></section>"#,
    )
}

/// The entity page, rendered from the finished view.
/// The fast half of the entity page: identity + capability only.
///
/// WHY THE PAGE IS SPLIT. Everything below the capability card needs every
/// source read, and an entity with real history can cost tens of seconds
/// because the page would be assembled whole before a byte is sent. No single
/// upstream is slow; the page simply waits for all of them. An operator
/// staring at a blank tab cannot tell that apart from a
/// hung service, which is the same "absence looks like something else"
/// failure this surface exists to prevent — just applied to itself.
///
/// So the shell paints from the spine alone and the timeline arrives after.
/// The spine is cheap: it is the identity resolution the reader needs first
/// anyway, and it is what tells them they are looking at the right entity.
pub fn entity_shell(site_label: &str, spine: &crate::spine::EntitySpine) -> String {
    // Takes the SPINE, not a built view — that is the whole point. Splitting only
    // the RENDERING would leave the handler still calling view::build(), so the
    // "fast" shell would wait for every adapter. A deferred region is not
    // deferred if the thing it defers has already been computed.
    let view = EntityView {
        capability: CapabilityPane {
            ory_resolved: spine.ory_resolved,
            anchors_resolved: spine.anchors_resolved,
            ory_groups: spine.ory_groups.clone(),
            write_anchors: spine.anchors.clone(),
            mesh_peers: spine.netbird_peers.clone(),
        },
        timeline: Vec::new(),
        sources: Vec::new(),
        silent_layers: Vec::new(),
        spine: spine.clone(),
    };
    shell_with(site_label, &entity_head(&view, true), &spine.selector)
}

/// The whole page in one document — no JS, nothing deferred. Kept because a
/// deferred fetch is a dependency, and `?full=1` (plus the noscript path)
/// must always be able to render everything server-side.
pub fn entity(site_label: &str, view: &EntityView) -> String {
    let mut b = String::new();
    b.push_str(&entity_head(view, false));
    b.push_str(&entity_body(view));
    return shell_with(site_label, &b, &view.spine.selector);
}

/// Hero + spine + capability. `deferred` swaps the timeline/sources region for
/// a placeholder that fetches itself.
fn entity_head(view: &EntityView, deferred: bool) -> String {
    let mut b = String::new();

    // --- hero: who -------------------------------------------------------
    let title = view
        .spine
        .handle
        .clone()
        .or_else(|| view.spine.email.clone())
        .unwrap_or_else(|| view.spine.selector.clone());
    b.push_str(&format!(
        r#"<section class="hero"><h1>{}</h1><p class="sel">selector: {}</p></section>"#,
        esc(&title),
        esc(&view.spine.selector)
    ));

    // --- spine + capability ---------------------------------------------
    b.push_str(r#"<section class="cards"><div class="card"><h2>Spine<span class="panel-kind">identity · point-in-time</span></h2><dl class="kv">"#);
    let mut dl = |k: &str, v: String| {
        if !v.is_empty() {
            b.push_str(&format!("<dt>{}</dt><dd>{}</dd>", esc(k), esc(&v)));
        }
    };
    dl("email", view.spine.email.clone().unwrap_or_default());
    dl("ory id", view.spine.ory_id.clone().unwrap_or_default());
    dl("anchors", view.spine.anchors.join(", "));
    dl("aliases", view.spine.aliases.join(", "));
    dl(
        "mesh peers (convention join)",
        view.spine.netbird_peers.join(", "),
    );
    // Kept as its own row, with its own wording. Folding these into "mesh
    // peers" would claim ownership the evidence does not support: this is
    // "the mesh audit records this entity acting on these peers", which an
    // administrator does to other people's machines routinely.
    // Three distinct relations, three rows. They were one row called
    // "administered", which put a person's own machines and other people's
    // machines they had merely re-grouped under the same heading.
    dl(
        "peers enrolled with a setup key you created",
        view.spine.netbird_peers_enrolled.join(", "),
    );
    dl(
        "peers administered (group/config changes, may be others' machines)",
        view.spine.netbird_peers_administered.join(", "),
    );
    for u in view
        .spine
        .unresolved
        .iter()
        .filter(|u| u.starts_with("no Ory identity for"))
    {
        b.push_str(&format!("<dt>ory identity</dt><dd>{}</dd>", esc(u)));
    }
    b.push_str("</dl>");
    // Diagnostics used to render HERE, as body text under the facts — so
    // "portal orgs unavailable" sat at the same visual weight as this
    // person's email. They are statements about the DEPLOYMENT, not about
    // the entity, and they now live with Sources where deployment health
    // belongs. The one exception stays: "no Ory identity for <email>" is a
    // fact about the subject.
    b.push_str("</div>");

    if let Some(m) = &view.spine.mesh {
        b.push_str(&mesh_card(m));
    }

    b.push_str(r#"<div class="card"><h2>Capability<span class="panel-kind">derived · point-in-time</span></h2><dl class="kv">"#);
    // "(none)" is a claim about the PERSON and may only be made when the
    // source answered. Unread renders as unknown, visibly marked, because an
    // identity-administration surface that reports "no groups" for a
    // never-queried Kratos is manufacturing a false negative.
    b.push_str(&format!(
        "<dt>Ory groups</dt><dd>{}</dd>",
        capability_value(
            view.capability.ory_resolved,
            &view.capability.ory_groups,
            "kratos not read",
        )
    ));
    b.push_str(&format!(
        "<dt>write anchors</dt><dd>{}</dd>",
        capability_value(
            view.capability.anchors_resolved,
            &view.capability.write_anchors,
            "portal not read",
        )
    ));
    b.push_str("</dl>");
    // Link to the second view. Rendered as a plain anchor so it works before
    // any client-side code exists and with JS disabled.
    b.push_str(&format!(
        r#"<p class="viewlink"><a href="/graph?id={}">mesh reachability graph →</a></p>"#,
        esc(&view.spine.selector)
    ));
    b.push_str("</div></section>");

    if deferred {
        // The placeholder is honest about its own state: "loading" is a claim
        // that a request is in flight, and the noscript path renders the whole
        // page server-side rather than leaving a reader with a permanent
        // spinner.
        b.push_str(&format!(
            r#"<section class="card card--live timeline" id="deferred" data-src="/entity/body?id={}">
<h2>Timeline<span class="panel-kind">event stream · newest first</span></h2>
<p class="empty">reading every source…</p>
<noscript><p class="empty">JavaScript is off — <a href="/entity?id={}&amp;full=1">load the full page</a> (slower: it waits for every source).</p></noscript>
</section>
<script>
(function(){{var e=document.getElementById('deferred');
fetch(e.dataset.src,{{credentials:'same-origin'}})
 .then(function(r){{if(!r.ok)throw new Error('HTTP '+r.status);return r.text()}})
 .then(function(h){{e.outerHTML=h}})
 .catch(function(err){{e.innerHTML='<h2>Timeline</h2><p class="empty">could not load the sources: '+err+' — <a href="/entity?id={}&amp;full=1">load the full page</a></p>'}});}})();
</script>"#,
            esc(&view.spine.selector),
            esc(&view.spine.selector),
            esc(&view.spine.selector),
        ));
        return b;
    }
    b
}

/// Timeline + sources + silent-layer banner — everything that needs all reads.
fn entity_body(view: &EntityView) -> String {
    entity_body_paged(view, 0, DEFAULT_PAGE)
}

/// Rows per page. A timeline of 150+ rows is not readable and is not free to
/// render; the operator asked for windows past ~20. Kept generous enough that
/// the common case is one page.
pub const DEFAULT_PAGE: usize = 50;

/// Public wrapper for the deferred fetch.
pub fn entity_body_public(view: &EntityView, offset: usize, limit: usize) -> String {
    entity_body_paged(view, offset, limit)
}

/// Whole page at an explicit window (the `full=1` / paged path).
pub fn entity_paged(site_label: &str, view: &EntityView, offset: usize, limit: usize) -> String {
    let mut b = entity_head(view, false);
    b.push_str(&entity_body_paged(view, offset, limit));
    shell_with(site_label, &b, &view.spine.selector)
}

fn entity_body_paged(view: &EntityView, offset: usize, limit: usize) -> String {
    let mut b = String::new();
    // --- timeline --------------------------------------------------------
    b.push_str(r#"<section class="card card--live timeline"><h2>Timeline<span class="panel-kind">event stream · newest first</span></h2>"#);
    if view.timeline.is_empty() {
        b.push_str(r#"<p class="empty">no attributed events from the ok sources above</p>"#);
    } else {
        // Unit in the header, following flashlight's `time (UTC)`. A bare
        // "when" on a fabric spanning timezones invites the reader to assume
        // local, and every adapter is normalized to UTC by timefmt.
        b.push_str(r#"<table class="table"><thead><tr><th>when (UTC)</th><th>kind</th><th>source</th><th>event</th></tr></thead><tbody>"#);
        let total = view.timeline.len();
        let end = (offset + limit).min(total);
        for e in &view.timeline[offset.min(total)..end] {
            let tag = match e.attribution {
                Attribution::Strong => "",
                Attribution::Convention => r#" <span class="attr">convention join</span>"#,
                Attribution::Coarse => r#" <span class="attr">coarse actor</span>"#,
            };
            // An unparseable source timestamp stays visibly unknown rather
            // than rendering as an empty cell the eye reads as "blank/none".
            let when = if e.ts.is_empty() {
                r#"<span class="unknown">unknown</span>"#.to_string()
            } else {
                esc(&e.ts)
            };
            // Kind first, because "did this person CHANGE something" is the
            // question a reader scans for; the source is provenance, secondary.
            let kind = e.kind.label();
            // An outcome the source RECORDED as bad outranks the kind badge:
            // a refused admin action and a successful one are not the same row.
            // Words come from Outcome::label so the vocabulary lives in one
            // place, the same reason EventKind::classify is a single function.
            let outcome_badge = match e.outcome {
                o @ (crate::event::Outcome::Refused | crate::event::Outcome::Failed) => format!(
                    r#" <span class="oc oc-{0}">{0}</span>"#,
                    o.label()
                ),
                _ => String::new(),
            };
            // Polarity sits beside kind, not inside it: "admin" says what sort
            // of act, "to them" says the entity was its OBJECT. An operator
            // resetting someone's groups and that person changing their own
            // are both admin, and a reader scanning for what this entity DID
            // needs to tell them apart at a glance.
            let dir_badge = match e.direction {
                crate::event::Direction::Undirected => String::new(),
                d => format!(
                    r#" <span class="dir dir-{}">{}</span>"#,
                    match d {
                        crate::event::Direction::BySubject => "by",
                        crate::event::Direction::AtSubject => "at",
                        crate::event::Direction::Reflexive => "self",
                        crate::event::Direction::Undirected => unreachable!(),
                    },
                    d.label()
                ),
            };
            let detail = match &e.detail {
                Some(d) if !d.is_empty() => format!(r#" <span class="err">{}</span>"#, esc(d)),
                _ => String::new(),
            };
            b.push_str(&format!(
                r#"<tr><td class="ts">{when}</td><td><span class="kind kind-{kind}">{kind}</span>{dir_badge}{outcome_badge}</td><td>{}</td><td>{}{tag}{detail}</td></tr>"#,
                esc(e.source),
                esc(&e.summary)
            ));
        }
        b.push_str("</tbody></table>");
        // Always state the window, even when it is the whole set: "showing 50"
        // with no denominator is how a truncated view gets mistaken for a
        // complete one, which on this surface is the same class of error as an
        // unread source rendering as "none".
        let shown_to = end;
        b.push_str(&format!(
            r#"<p class="pager">showing {}–{} of {} event(s)"#,
            if total == 0 { 0 } else { offset + 1 },
            shown_to,
            total
        ));
        if offset > 0 {
            b.push_str(&format!(
                r#" · <a href="/entity?id={}&amp;full=1&amp;offset={}">newer</a>"#,
                esc(&view.spine.selector),
                offset.saturating_sub(limit)
            ));
        }
        if end < total {
            b.push_str(&format!(
                r#" · <a href="/entity?id={}&amp;full=1&amp;offset={}">older</a>"#,
                esc(&view.spine.selector),
                offset + limit
            ));
        }
        b.push_str("</p>");
    }
    b.push_str("</section>");

    // --- sources honesty row --------------------------------------------
    b.push_str(r#"<section class="card sources"><h2>Sources<span class="panel-kind">status · as of this page load</span></h2><table class="table"><thead><tr><th>stream</th><th>state</th><th>n</th><th>detail</th></tr></thead><tbody>"#);
    for s in &view.sources {
        let cls = match s.state {
            SourceState::Ok => "ok",
            SourceState::Skipped => "skipped",
            SourceState::Error => "error",
        };
        b.push_str(&format!(
            r#"<tr class="{cls}"><td>{}</td><td>{cls}</td><td>{}</td><td>{}</td></tr>"#,
            esc(s.source),
            s.n,
            esc(&s.detail)
        ));
    }
    b.push_str("</tbody></table>");
    // Deployment-level diagnostics, relocated out of Spine. Same strings,
    // same information — attached to the health surface instead of sitting
    // among facts about a person.
    let deployment_notes: Vec<&String> = view
        .spine
        .unresolved
        .iter()
        .filter(|u| !u.starts_with("no Ory identity for"))
        .collect();
    if !deployment_notes.is_empty() {
        b.push_str(r#"<div class="diagnostics"><h3>Join diagnostics</h3><ul>"#);
        for u in deployment_notes {
            b.push_str(&format!("<li>{}</li>", esc(u)));
        }
        b.push_str("</ul></div>");
    }
    if !view.silent_layers.is_empty() {
        b.push_str(&format!(
            r#"<p class="silent">silently-refusing layers (no denial records exist): {} — absence of a record here is NOT evidence of success.</p>"#,
            esc(&view.silent_layers.join(", "))
        ));
    }
    b.push_str("</section>");

    b
}

/// 403 page — the gate refused. Explicit, never blank (and the refusal is
/// itself logged by the caller).
pub fn forbidden(site_label: &str, groups_seen: &str) -> String {
    shell(
        site_label,
        &format!(
            r#"<section class="hero"><h1>403</h1>
<p>microscope is an identity-administration surface — admission requires one
of the configured admin groups; your session carries: <code>{}</code>.</p>
<p>A data-access grant does not admit this surface by design.</p></section>"#,
            esc(if groups_seen.is_empty() { "(none)" } else { groups_seen })
        ),
    )
}

/// The reachability graph page — server-rendered SVG.
///
/// Rendered here rather than handed to a client-side library so that the
/// markup-asserting tests cover it (that discipline caught both kind/outcome
/// defects before they shipped) and so the page draws with JavaScript off.
/// Links are real `<a>` elements: clickthrough works before any D3 lands.
pub fn graph_page(
    site_label: &str,
    selector: &str,
    g: &crate::graph::ReachabilityGraph,
    spine: &crate::spine::EntitySpine,
    focus: Option<&str>,
    show_all: bool,
) -> String {
    use crate::graph::{EdgeState, NodeKind};
    let mut b = String::new();
    // Back link at the TOP. It also existed at the bottom, which on a page
    // this tall meant scrolling past the whole graph to leave it.
    b.push_str(&format!(
        r#"<p class="crumb"><a href="/entity?id={}">← back to timeline</a></p>"#,
        esc(selector)
    ));
    b.push_str(&format!(
        r#"<section class="hero"><h1>{}</h1><p class="sel">mesh reachability · NetBird layer only{}</p></section>"#,
        esc(selector),
        focus.map(|f| format!(" · subject: {}", esc(f))).unwrap_or_default()
    ));

    // The legend states the honesty contract up front, because the picture
    // cannot carry it. "Permitted" is complete HERE and nowhere else.
    b.push_str(r#"<section class="card"><h2>What this shows<span class="panel-kind">policy · point-in-time</span></h2>
<ul class="legend">
<li><span class="ek ek-reachable"></span> permitted <em>and</em> both ends have a peer up now — reachable, <strong>not</strong> observed in use</li>
<li><span class="ek ek-permitted"></span> permitted by an enabled policy rule</li>
<li>click a path to see the policy and group pair that permits it</li>
<li><strong>a line is not unrestricted access</strong> — every path carries its protocol and port limits in the table below, and its direction. A broad-looking fan of lines limited to ICMP is a very different posture from the same fan on all ports</li>
</ul>
<p class="note">Each line is a path from one of this entity's peers (left) to a peer it may reach (right). Groups and policies are the <em>mechanism</em>, not the path, so they are not drawn — they appear when you select a line. Because the NetBird policy set is fully enumerable, a path that is <strong>not</strong> drawn really is denied; that is <em>not</em> true of the fabric layer, where denial is silent.</p>
</section>"#);

    // No subject chosen: offer the machines this entity is connected to,
    // rather than drawing all of them at once (which is what made the last
    // two versions unreadable).
    if focus.is_none() && !show_all {
        let mut cands: Vec<&String> = spine
            .netbird_peers
            .iter()
            .chain(spine.netbird_peers_enrolled.iter())
            .chain(spine.netbird_peers_administered.iter())
            .collect();
        cands.sort();
        cands.dedup();
        let n_cands = cands.len();
        b.push_str(r#"<section class="card"><h2>Pick a machine<span class="panel-kind">subject · one at a time</span></h2>
<p class="note">Reachability is a property of a MACHINE, not of a person — a person has no single position in the mesh. Choose one to see what it can reach.</p>"#);
        if cands.is_empty() {
            b.push_str(r#"<p class="empty">no machine is associated with this entity by either join</p>"#);
        } else {
            b.push_str(r#"<ul class="pick">"#);
            for c in cands {
                b.push_str(&format!(
                    r#"<li><a href="/graph?id={}&amp;peer={}">{}</a></li>"#,
                    esc(selector),
                    esc(c),
                    esc(c)
                ));
            }
            b.push_str("</ul>");
            b.push_str(&format!(
                r#"<p class="note">Or <a href="/graph?id={}&amp;all=1">show all {} machines at once</a> — useful for "what can this whole estate reach", unreadable as a default.</p>"#,
                esc(selector),
                n_cands
            ));
        }
        b.push_str("</section>");
        return shell_with(site_label, &b, selector);
    }
    if g.nodes.is_empty() {
        b.push_str(r#"<section class="card"><p class="empty">no graph</p><ul class="notes">"#);
        for n in &g.notes {
            b.push_str(&format!("<li>{}</li>", esc(n)));
        }
        b.push_str("</ul></section>");
        return shell(site_label, &b);
    }

    let height = g.nodes.iter().fold(160.0_f64, |m, n| m.max(n.y + 70.0));
    b.push_str(&format!(
        r#"<section class="card card--live graph"><h2>Reachability<span class="panel-kind">derived · point-in-time</span></h2>
<div class="graph-scroll"><svg viewBox="0 0 900 {height}" width="100%" height="{height}" role="img" aria-label="mesh reachability graph">"#
    ));

    let pos = |id: &str| -> Option<(f64, f64)> {
        g.nodes.iter().find(|n| n.id == id).map(|n| (n.x, n.y))
    };
    // Edges first so nodes paint over them.
    for e in &g.edges {
        if let (Some((x1, y1)), Some((x2, y2))) = (pos(&e.from), pos(&e.to)) {
            let mid = (x1 + x2) / 2.0;
            let title = match &e.via {
                Some(v) => format!("{} — via {}", e.state.label(), v),
                None => e.state.label().to_string(),
            };
            b.push_str(&format!(
                r#"<path class="edge edge-{cls}" tabindex="0" role="button" d="M{x1} {y1} C{mid} {y1} {mid} {y2} {x2} {y2}" data-detail="{d}"><title>{t}</title></path>"#,
                cls = e.state.css(),
                t = esc(&title),
                d = esc(&format!("{} → {}  ·  {}", e.from.splitn(2,':').nth(1).unwrap_or(""), e.to.splitn(2,':').nth(1).unwrap_or(""), title)),
            ));
        }
    }
    for n in &g.nodes {
        let (cls, sub) = match n.kind {
            NodeKind::Peer => (
                "peer",
                match n.connected {
                    Some(true) => "up".to_string(),
                    Some(false) => "down".to_string(),
                    None => String::new(),
                },
            ),
            NodeKind::SourceGroup | NodeKind::DestGroup => (
                if matches!(n.kind, NodeKind::SourceGroup) { "srcgroup" } else { "dstgroup" },
                match (n.members, n.members_connected) {
                    (Some(t), Some(c)) => format!("{c}/{t} up"),
                    _ => String::new(),
                },
            ),
        };
        b.push_str(&format!(
            r#"<g class="node node-{cls}" transform="translate({x},{y})"><rect x="-84" y="-17" width="168" height="34" rx="5"></rect><text class="nlabel" x="0" y="-1">{label}</text><text class="nsub" x="0" y="12">{sub}</text></g>"#,
            x = n.x,
            y = n.y,
            label = esc(&elide(&n.label, 24)),
            sub = esc(&sub),
        ));
    }
    b.push_str("</svg></div>");

    // WHY each path exists, on the page rather than behind a hover. A graph
    // that shows only the result is an assertion; the operator's next action
    // is almost always to change a group or a rule, and they cannot do that
    // from a picture of arrows.
    if !g.subject_groups.is_empty() {
        b.push_str(&format!(
            r#"<p class="subjgroups"><strong>This machine's groups:</strong> {}</p>"#,
            esc(&g.subject_groups.join(", "))
        ));
    }
    let explained: Vec<&crate::graph::Edge> =
        g.edges.iter().filter(|e| !e.rules.is_empty()).collect();
    if !explained.is_empty() {
        b.push_str(r#"<h3 class="whyhead">What permits each path</h3>
<div class="graph-scroll"><table class="table"><thead><tr><th>to</th><th>direction</th><th>protocol · ports</th><th>state</th><th>permitted by (policy)</th><th>via (group → group)</th></tr></thead><tbody>"#);
        for e in &explained {
            let to = e.to.splitn(2, ':').nth(1).unwrap_or("");
            // Direction is its own column because A→B and A↔B are different
            // grants and an arrow alone cannot tell them apart. One-way is
            // called out, not merely implied by the absence of a second arrow.
            let (dir_cls, dir_txt) = if e.bidirectional {
                ("dir-both", "↔ both ways")
            } else {
                ("dir-oneway", "→ outbound only")
            };
            // The constraint comes from the RULE. Policy names carry strings
            // like "(icmp)" that look like limits and are only labels.
            let cons = if e.constraints.is_empty() {
                "unstated".to_string()
            } else {
                e.constraints.join(", ")
            };
            b.push_str(&format!(
                r#"<tr><td>{}</td><td><span class="dirc {}">{}</span></td><td>{}</td><td><span class="oc oc-{}">{}</span></td><td>{}</td><td>{}</td></tr>"#,
                esc(to),
                dir_cls,
                esc(dir_txt),
                esc(&cons),
                e.state.css(),
                esc(e.state.label()),
                esc(&e.rules.join(", ")),
                esc(&e.via_groups.join(", ")),
            ));
        }
        b.push_str("</tbody></table></div>");
    }
    // Selecting a path shows what permits it. Server-rendered <title> already
    // carries the same text, so hover works with JS off; this only upgrades it
    // to a persistent panel.
    b.push_str(r#"<p class="pathdetail" id="pathdetail">select a path to see what permits it</p>
<script>(function(){var p=document.getElementById('pathdetail');
document.querySelectorAll('path.edge').forEach(function(el){
  function show(){p.textContent=el.dataset.detail;p.classList.add('on');
    document.querySelectorAll('path.edge').forEach(function(o){o.classList.remove('sel')});el.classList.add('sel');}
  el.addEventListener('click',show);el.addEventListener('focus',show);});})();</script>"#);

    b.push_str(r#"<ul class="notes">"#);
    for n in &g.notes {
        b.push_str(&format!("<li>{}</li>", esc(n)));
    }
    b.push_str("</ul></section>");
    let _ = EdgeState::Permitted;
    shell_with(site_label, &b, selector)
}

/// Truncate a label for the fixed-width node box without lying about it —
/// an elided name ends in an ellipsis so nobody reads it as the whole name.
fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The mesh forensics card: what the selector matched, who is behind it, what
/// they have changed, and what currently admits traffic to them.
///
/// Rendered as four blocks in that order because that is the order the
/// question gets asked in an incident — "what is this thing", "whose is it",
/// "what have they been doing", "what can reach it".
fn mesh_card(m: &crate::mesh::MeshView) -> String {
    let mut b = String::new();
    let kind = match m.kind {
        Some(crate::mesh::SelectorKind::MeshIp) => "mesh IP",
        Some(crate::mesh::SelectorKind::PeerName) => "peer name",
        Some(crate::mesh::SelectorKind::NetbirdAccount) => "NetBird account id",
        Some(crate::mesh::SelectorKind::NoMeshMatch) => "no mesh match",
        None => "not resolved",
    };
    b.push_str(&format!(
        r#"<div class="card"><h2>Mesh<span class="panel-kind">recorded · selector read as {}</span></h2>"#,
        esc(kind)
    ));

    if !m.peers.is_empty() {
        b.push_str(r#"<table class="grid"><thead><tr><th>peer</th><th>mesh ip</th><th>state</th><th>account</th><th>attribution</th></tr></thead><tbody>"#);
        for p in &m.peers {
            // "ephemeral" is stated because it changes how every other cell
            // should be read: the peer is expected to vanish, so `offline` is
            // not a fault and its id will never be reused.
            let state = if p.connected {
                "connected".to_string()
            } else if p.ephemeral {
                format!("gone (ephemeral) · last seen {}", p.last_seen)
            } else {
                format!("offline · last seen {}", p.last_seen)
            };
            b.push_str(&format!(
                "<tr><td>{}</td><td class=\"mono\">{}</td><td>{}</td><td class=\"mono\">{}</td><td>{}{}</td></tr>",
                esc(&p.name),
                esc(&p.ip),
                esc(&state),
                esc(p.account_id.as_deref().unwrap_or("—")),
                esc(&p.attribution),
                p.via_setup_key
                    .as_ref()
                    .map(|k| format!(" · via key {}", esc(k)))
                    .unwrap_or_default(),
            ));
        }
        b.push_str("</tbody></table>");
    }

    for a in &m.accounts {
        let who = a.account_name.clone().unwrap_or_else(|| a.account_id.clone());
        b.push_str(&format!(
            r#"<h3 class="sub">{}{} · role {} · last login {}</h3>"#,
            esc(&who),
            if a.is_service_user { " (service user)" } else { "" },
            esc(if a.role.is_empty() { "unknown" } else { &a.role }),
            esc(if a.last_login.starts_with("0001-") {
                "never"
            } else {
                &a.last_login
            }),
        ));
        if a.buckets.is_empty() {
            // A zero MUST carry its window, or it reads as "this account has
            // never changed anything" when it means "not in the log we hold".
            b.push_str(&format!(
                "<p class=\"muted\">no configuration writes in the activity log ({} → {})</p>",
                esc(&a.window_first),
                esc(&a.window_last)
            ));
            continue;
        }
        b.push_str(r#"<table class="grid"><thead><tr><th>class</th><th>writes</th><th>last</th><th>objects</th></tr></thead><tbody>"#);
        for k in &a.buckets {
            let mut objs = k.targets.join(", ");
            if let Some(n) = k.targets_truncated {
                objs.push_str(&format!(" … and {n} more"));
            }
            b.push_str(&format!(
                "<tr><td>{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td>{}</td></tr>",
                esc(&k.class),
                k.count,
                esc(&k.last),
                esc(&objs)
            ));
        }
        b.push_str("</tbody></table>");
    }

    if !m.grants.is_empty() {
        b.push_str(r#"<h3 class="sub">what admits traffic to these peers</h3>"#);
        b.push_str(r#"<table class="grid"><thead><tr><th>policy</th><th>sources</th><th>via</th><th>constraint</th><th>last write</th></tr></thead><tbody>"#);
        for g in &m.grants {
            // The constraint comes from the RULE. A policy NAMED "SSH Access"
            // whose rule permits every protocol on every port is the case this
            // column exists to expose, so the widest rows are flagged and sort
            // first rather than being left for a reader to notice.
            b.push_str(&format!(
                "<tr><td>{}{}</td><td>{}</td><td>{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td></tr>",
                esc(&g.policy_name),
                if g.unrestricted && g.enabled {
                    " <strong>UNRESTRICTED</strong>"
                } else if !g.enabled {
                    " (disabled)"
                } else {
                    ""
                },
                esc(&g.sources),
                esc(&g.via),
                esc(&g.constraint),
                esc(g.last_write.as_deref().unwrap_or("not in window")),
            ));
        }
        b.push_str("</tbody></table>");
    }

    b.push_str("</div>");
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{SourceState, SourceStatus, TimelineEvent};
    use crate::spine::EntitySpine;
    use crate::view::{CapabilityPane, EntityView};

    // There were NO render tests earlier, which is a large part of
    // why a whole source rendered blank timestamps in production for weeks
    // without anything going red. These pin the HONESTY properties, not the
    // styling: a redesign should be free to move markup, and must not be free
    // to make the page claim something it does not know.

    fn view(capability: CapabilityPane, timeline: Vec<TimelineEvent>) -> EntityView {
        EntityView {
            spine: EntitySpine {
                selector: "someone@example.invalid".into(),
                ..Default::default()
            },
            capability,
            timeline,
            sources: vec![SourceStatus {
                source: "kratos-sessions",
                state: SourceState::Skipped,
                detail: "MICROSCOPE_KRATOS_ADMIN_URL unset".into(),
                n: 0,
            }],
            silent_layers: vec!["zenoh-write"],
        }
    }

    fn cap(ory_resolved: bool, groups: &[&str]) -> CapabilityPane {
        CapabilityPane {
            ory_groups: groups.iter().map(|s| s.to_string()).collect(),
            write_anchors: vec![],
            mesh_peers: vec![],
            ory_resolved,
            anchors_resolved: false,
        }
    }

    fn event(ts: &str) -> TimelineEvent {
        TimelineEvent {
            ts: ts.into(),
            source: "portal-audit",
            action: "vendor.create".into(),
            actor: "operator".into(),
            target: None,
            summary: "created org".into(),
            kind: crate::event::EventKind::Admin,
            outcome: crate::event::Outcome::Unrecorded,
            direction: crate::event::Direction::BySubject,
            detail: None,
            attribution: Attribution::Coarse,
            raw: None,
        }
    }

    #[test]
    fn unread_kratos_never_renders_none_for_groups() {
        // The false-negative guard. "(none)" is a claim about the PERSON; an
        // unread source must not produce it, or an admin concludes someone is
        // unprivileged when we simply never asked.
        let html = entity("zagros-prod", &view(cap(false, &[]), vec![]));
        assert!(
            !html.contains("<dt>Ory groups</dt><dd>(none)</dd>"),
            "unread Kratos must not render (none)"
        );
        assert!(html.contains("kratos not read"), "must say why: {html}");
    }

    #[test]
    fn read_kratos_with_no_groups_does_render_none() {
        // The other half — when we DID ask, "(none)" is the honest answer and
        // must not be softened into "unknown".
        let html = entity("zagros-prod", &view(cap(true, &[]), vec![]));
        assert!(html.contains("<dt>Ory groups</dt><dd>(none)</dd>"), "{html}");
    }

    #[test]
    fn groups_render_when_present() {
        let html = entity("zagros-prod", &view(cap(true, &["PLATFORM-ADMIN"]), vec![]));
        assert!(html.contains("PLATFORM-ADMIN"));
        // Assert on the rendered cell, not a bare substring: the literal
        // "(none)" also appears in an embedded CSS comment, and a test that
        // greps the whole document would pass or fail for the wrong reason.
        assert!(
            !html.contains("<dt>Ory groups</dt><dd>(none)</dd>"),
            "a present group list must not also render (none)"
        );
    }

    #[test]
    fn undated_event_renders_unknown_not_an_empty_cell() {
        // The portal-audit symptom: a blank <td> reads as "nothing here"
        // rather than "we could not place this in time".
        let html = entity("zagros-prod", &view(cap(true, &[]), vec![event("")]));
        assert!(html.contains(">unknown<"), "{html}");
        assert!(!html.contains(r#"<td class="ts"></td>"#), "no blank ts cells");
    }

    #[test]
    fn dated_event_renders_its_own_timestamp_verbatim() {
        // microscope.md: events carry their own timestamps, never re-stamped.
        let html = entity("zagros-prod", &view(cap(true, &[]), vec![event("2026-08-07T06:21:48.000Z")]));
        assert!(html.contains("2026-08-07T06:21:48.000Z"), "{html}");
    }

    #[test]
    fn time_column_declares_its_unit() {
        let html = entity("zagros-prod", &view(cap(true, &[]), vec![event("2026-08-07T06:21:48.000Z")]));
        assert!(html.contains("when (UTC)"), "a bare 'when' invites a local-time misread");
    }

    #[test]
    fn every_source_state_is_rendered_with_its_reason() {
        let html = entity("zagros-prod", &view(cap(true, &[]), vec![]));
        assert!(html.contains("kratos-sessions"));
        assert!(html.contains("skipped"));
        assert!(html.contains("MICROSCOPE_KRATOS_ADMIN_URL unset"), "reason must survive");
    }

    #[test]
    fn silent_layer_banner_keeps_its_absence_of_evidence_wording() {
        // The strongest honesty string on the page — a redesign must not
        // quietly drop it.
        let html = entity("zagros-prod", &view(cap(true, &[]), vec![]));
        assert!(html.contains("absence of a record here is NOT evidence of success"));
    }

    #[test]
    fn empty_timeline_scopes_its_claim_to_ok_sources() {
        let html = entity("zagros-prod", &view(cap(true, &[]), vec![]));
        assert!(html.contains("no attributed events from the ok sources above"));
    }

    #[test]
    fn timeline_precedes_sources() {
        // The artifact an operator came for outranks the meter that
        // describes how it was gathered.
        let html = entity("zagros-prod", &view(cap(true, &[]), vec![]));
        assert!(html.find("<h2>Timeline").unwrap() < html.find("<h2>Sources").unwrap(), "timeline must render first");
    }

    #[test]
    fn deployment_diagnostics_leave_the_spine_card() {
        let mut v = view(cap(true, &[]), vec![]);
        v.spine.unresolved = vec![
            "portal orgs unavailable: portal client disabled".into(),
            "no Ory identity for a@b.invalid".into(),
        ];
        let html = entity("zagros-prod", &v);
        // Assert on MARKUP, not a bare substring: the embedded stylesheet
        // quotes this phrase in a comment, and a whole-document grep would
        // pass or fail for the wrong reason.
        let li = "<li>portal orgs unavailable: portal client disabled</li>";
        assert!(html.contains(li), "diagnostic text must survive the move");
        assert!(html.contains("Join diagnostics"));
        assert!(html.find(li).unwrap() > html.find("<h2>Timeline").unwrap());
        // The entity-level one stays in the Spine content where it belongs.
        assert!(html.find("no Ory identity for a@b.invalid").unwrap() < html.find("<h2>Timeline").unwrap());
    }

    #[test]
    fn panels_declare_whether_they_are_streams_or_state() {
        let html = entity("zagros-prod", &view(cap(true, &[]), vec![]));
        assert!(html.contains("event stream"), "timeline must say it is a stream");
        assert!(html.contains("point-in-time"), "state panels must say so");
        assert!(html.contains("card--live"), "stream uses the design-system live treatment");
    }
}

#[cfg(test)]
mod nav_tests {
    use super::*;

    /// Search must be reachable from EVERY page, not just the landing page.
    /// It was landing-page-only, so after examining one entity the only way to
    /// examine another was to hand-edit the URL.
    #[test]
    fn every_page_carries_a_search_box() {
        let page = shell("zagros-prod", "<p>body</p>");
        assert!(page.contains(r#"class="navsearch""#));
        assert!(page.contains(r#"action="/entity""#));
    }

    /// The box is pre-filled so a reader edits the selector they are on
    /// instead of retyping it.
    #[test]
    fn the_search_box_carries_the_current_selector() {
        let page = shell_with("zagros-prod", "<p>b</p>", "someone@example.test");
        assert!(page.contains(r#"value="someone@example.test""#));
    }

    /// The typeahead ships with every page's search box, including the
    /// landing picker — one behaviour, not two.
    #[test]
    fn every_page_carries_the_typeahead() {
        for page in [shell("zagros-prod", "<p>body</p>"), home("zagros-prod")] {
            assert!(page.contains("/api/suggest?limit="), "suggest fetch");
            assert!(page.contains("form.navsearch input[name=id]"), "top-bar box");
            assert!(page.contains("form.picker input[name=id]"), "landing box");
        }
    }

    /// The form still submits with JS off: the typeahead wraps a plain GET
    /// form, it does not replace it.
    #[test]
    fn the_box_still_works_without_javascript() {
        let page = shell("zagros-prod", "<p>body</p>");
        assert!(page.contains(r#"<form class="navsearch" action="/entity" method="get""#));
        assert!(page.contains(r#"<button type="submit">examine</button>"#));
    }

    /// Suggestion labels are written with textContent and the endpoint's
    /// answer is never interpolated as markup — the one property that keeps a
    /// roster of operator-supplied names from becoming script.
    #[test]
    fn suggestions_are_built_as_text_not_markup() {
        let page = shell("zagros-prod", "<p>body</p>");
        assert!(page.contains("textContent"));
        assert!(
            !page.contains("innerHTML=") || !page.contains("m.label"),
            "a suggestion label must never reach innerHTML"
        );
    }

    /// A selector containing markup must not break out of the value attribute.
    #[test]
    fn the_prefill_is_escaped() {
        let page = shell_with("zagros-prod", "<p>b</p>", r#""><script>x</script>"#);
        assert!(!page.contains("<script>x</script>"));
        assert!(page.contains("&quot;&gt;&lt;script&gt;"));
    }
}
