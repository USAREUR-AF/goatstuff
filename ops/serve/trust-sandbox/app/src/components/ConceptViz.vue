<script setup lang="ts">
// Per-concept anatomy + mechanics — rendered as HTML/SVG (real text that wraps
// and never overflows) with CSS-animated accents. One bespoke diagram per
// aspect, chosen by `gate`.
defineProps<{ gate: string }>()
</script>

<template>
  <div class="anat">
    <!-- ① NetBird — admission: setup key → a LOCAL profile + credentials → the client daemon -->
    <div v-if="gate==='netbird'" class="cert-wrap">
      <div class="cert-left">
        <div class="node"><div class="nt">Device + setup key</div><div class="ns">a one-time enrollment token</div></div>
        <div class="signs">enrolls ↓</div>
        <div class="node amber"><div class="nt">Management</div><div class="ns">validates the key, registers the peer</div></div>
        <div class="signs">profile written locally ↓</div>
      </div>
      <div class="cert-card pulse">
        <div class="cc-title">the NetBird profile — generated + stored on the device</div>
        <div class="cc-row"><span class="cc-k">WireGuard private key</span><span class="cc-v">Curve25519, on the device</span><span class="cc-a">never leaves the machine</span></div>
        <div class="cc-row"><span class="cc-k">public key</span><span class="cc-v">sent to Management</span><span class="cc-a">how other peers find + verify you</span></div>
        <div class="cc-row"><span class="cc-k">mesh IP</span><span class="cc-v">100.x, assigned</span><span class="cc-a">your address on the overlay</span></div>
        <div class="cc-row"><span class="cc-k">peers + routes</span><span class="cc-v">pushed by Management</span><span class="cc-a">who you may reach</span></div>
        <div class="cc-row sig"><span class="cc-k">stored at</span><span class="cc-v">~/.netbird (config + state)</span><span class="cc-a">read by the client daemon</span></div>
      </div>
      <div class="cert-mtls">
        <div class="mt-pair"><div class="node sm">netbird client</div><div class="mt-arrows"><span>⇢</span></div><div class="node sm ring">the mesh</div></div>
        <div class="mt-cap">the <b>netbird client</b> (daemon) loads the local profile and brings up the <b>WireGuard</b> tunnel to its peers — this is <b>admission, not trust</b></div>
      </div>
    </div>

    <!-- ② x509 — the certificate, taken apart -->
    <div v-else-if="gate==='cert'" class="cert-wrap">
      <div class="cert-left">
        <div class="node amber"><div class="nt">Certificate Authority</div><div class="ns">the trusted signer</div></div>
        <div class="signs">signs ↓</div>
      </div>
      <div class="cert-card pulse">
        <div class="cc-title">an X.509 certificate, taken apart</div>
        <div class="cc-row"><span class="cc-k">Subject</span><span class="cc-v">CN = acme-sensors</span><span class="cc-a">the Common Name (CN) — your access anchor</span></div>
        <div class="cc-row"><span class="cc-k">Subject Alt Name</span><span class="cc-v">urn:goat:__goatnet__:org=acme-sensors,role=OPERATOR</span><span class="cc-a">the identity grammar, in the SAN</span></div>
        <div class="cc-row"><span class="cc-k">Issuer</span><span class="cc-v">trust-sandbox CA</span><span class="cc-a">who vouched for it</span></div>
        <div class="cc-row"><span class="cc-k">Validity</span><span class="cc-v">short-lived · auto-expires</span><span class="cc-a">it lapses on its own</span></div>
        <div class="cc-row"><span class="cc-k">Public key</span><span class="cc-v">the holder’s keypair</span><span class="cc-a">proves possession on connect</span></div>
        <div class="cc-row sig"><span class="cc-k">CA signature</span><span class="cc-v">signs everything above</span><span class="cc-a">makes it unforgeable</span></div>
      </div>
      <div class="cert-mtls">
        <div class="mt-pair"><div class="node sm">client</div><div class="mt-arrows"><span>⇄</span></div><div class="node sm">server</div></div>
        <div class="mt-cap"><b>mutual TLS</b><br />both present a cert and verify the other — neither is trusted by address</div>
      </div>
    </div>

    <!-- ③ SPIRE — two-step attestation → a short-lived, auto-rotating SVID, taken apart -->
    <div v-else-if="gate==='spire'" class="cert-wrap">
      <div class="cert-left">
        <div class="node accent"><div class="nt">SPIRE server</div><div class="ns">the issuer + trust root</div></div>
        <div class="signs">① attests the node ↓</div>
        <div class="node accent"><div class="nt">Agent</div><div class="ns">on the node — proves the host</div></div>
        <div class="signs">② attests the workload ↓</div>
        <div class="node accent"><div class="nt">Workload</div><div class="ns">proven by uid / path / selector</div></div>
      </div>
      <div class="cert-card pulse">
        <div class="cc-title">an SVID — a SPIFFE Verifiable Identity Document (an X.509 cert)</div>
        <div class="cc-row"><span class="cc-k">SPIFFE ID</span><span class="cc-v">spiffe://trust.sandbox/workload/edge-processor</span><span class="cc-a">the workload's name, in the URI-SAN</span></div>
        <div class="cc-row"><span class="cc-k">CN</span><span class="cc-v">edge-processor</span><span class="cc-a">the access anchor — matches the Zenoh ACL</span></div>
        <div class="cc-row"><span class="cc-k">Issuer</span><span class="cc-v">the SPIRE server CA</span><span class="cc-a">the attestation trust root</span></div>
        <div class="cc-row"><span class="cc-k">Validity</span><span class="cc-v">~60s · auto-rotates</span><span class="cc-a">a leaked one is useless in minutes</span></div>
        <div class="cc-row"><span class="cc-k">Key</span><span class="cc-v">generated by the agent</span><span class="cc-a">the workload never holds a long-lived secret</span></div>
        <div class="cc-row sig"><span class="cc-k">how it's earned</span><span class="cc-v">attestation, not configuration</span><span class="cc-a">no password, no copied key</span></div>
      </div>
      <div class="cert-mtls">
        <div class="ttl">
          <div class="ttl-ring"><span class="ttl-fill" /></div>
          <div class="ttl-cap">the agent <b>auto-rotates</b> the SVID continuously — <b>no human in the loop</b></div>
        </div>
      </div>
    </div>

    <!-- ④ Ory — human authN: OIDC login → a signed JWT (its own drill-down) → role-gated access -->
    <div v-else-if="gate==='ory'" class="flow">
      <div class="node"><div class="nt">Human</div><div class="ns">a person at a browser</div></div>
      <div class="conn"><span class="cl">OAuth2 / OIDC login</span><span class="ca">→</span></div>
      <div class="node accent"><div class="nt">Ory — Kratos + Hydra</div><div class="ns">checks the login, issues a token</div></div>
      <div class="conn"><span class="ca">→</span></div>
      <div class="node amber"><div class="nt">a signed JWT</div><div class="ns">who you are + your role</div></div>
      <div class="conn flow-anim"><span class="cl green-t">read by web services</span><span class="ca">⇢</span></div>
      <div class="node ring"><div class="nt">role-gated access</div><div class="ns">view · operate · admin</div></div>
    </div>

    <!-- ⑤ Zenoh ACL — PEP on the path, PDP decides by matching the CN; default-deny -->
    <div v-else-if="gate==='acl'" class="acl-wrap">
      <div class="acl-top">
        <div class="node"><div class="nt">a write</div><div class="ns">carries its Common Name<br /><span class="mono">CN = acme-sensors</span></div></div>
        <div class="conn"><span class="ca">→</span></div>
        <div class="pep"><div class="pep-bar" /><div class="pep-l">Enforcement point<br /><span>(PEP) · on the path</span></div></div>
        <div class="acl-branch">
          <div class="branch ok"><span class="ca">→</span><div class="node green sm">subscriber · write lands</div></div>
          <div class="branch no"><span class="ca">✕</span><div class="node pink sm">a wrong Common Name → silently dropped<br /><span class="ns">(no error returns)</span></div></div>
        </div>
      </div>
      <div class="acl-pdp">
        <div class="updash" />
        <div class="node accent sm">Policy decision point (PDP) · <b>default-DENY</b><div class="ns">match the Common Name → a granted anchor?</div></div>
      </div>
    </div>

    <div class="anat-note">
      <template v-if="gate==='netbird'">Admission is established <b>once</b> at onboarding — independent of the identity below. Being on the mesh is not trust.</template>
      <template v-else-if="gate==='cert'">A signed identity you present <b>per connection</b> — a scoped, time-limited, revocable keycard.</template>
      <template v-else-if="gate==='spire'">SPIFFE is a standard for workload identity; SPIRE implements it. Identity is <b>earned by attestation</b> and re-proven continuously — a leaked credential is useless within minutes.</template>
      <template v-else-if="gate==='ory'">Ory authenticates the human + role — it does <b>not</b> issue. A role authorizes admin actions, like a CA mint for a machine. SPIRE issues workload identities automatically by attestation — <b>no human in the loop</b>.</template>
      <template v-else-if="gate==='acl'">The per-write decision, <b>keyed to the identity</b> above. A denied write is silent — the sender is told nothing.</template>
    </div>
  </div>
</template>

<style scoped>
.anat { padding: 6px 2px; }
.flow { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; justify-content: center; padding: 14px 0; }
.node { background: linear-gradient(180deg, var(--panel), var(--panel-2)); border: 1.4px solid var(--line); border-radius: 10px;
  padding: 12px 16px; text-align: center; min-width: 110px; }
.node.sm { padding: 8px 12px; min-width: 0; font-size: 12px; }
.node.accent { border-color: color-mix(in srgb, var(--accent) 55%, var(--line)); }
.node.amber { border-color: color-mix(in srgb, var(--amber) 55%, var(--line)); }
.node.green { border-color: color-mix(in srgb, var(--green) 55%, var(--line)); }
.node.pink { border-color: color-mix(in srgb, var(--pink) 55%, var(--line)); }
.node.ring { border-style: dashed; border-radius: 999px; }
.node .nt { font-weight: 600; font-size: 13.5px; color: var(--ink); }
.node .ns { font-size: 11px; color: var(--ink-dim); margin-top: 2px; }
.node .ns.mono, .mono { font-family: var(--font-mono); }
.pulse { box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 40%, transparent); animation: pulse 2.4s var(--ease) infinite; }
@keyframes pulse { 0%,100% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 35%, transparent); } 50% { box-shadow: 0 0 16px -2px color-mix(in srgb, var(--accent) 55%, transparent); } }
.conn { display: flex; flex-direction: column; align-items: center; min-width: 60px; }
.conn .cl { font-family: var(--font-mono); font-size: 9.5px; color: var(--ink-faint); letter-spacing: .03em; text-align: center; max-width: 96px; line-height: 1.3; }
.conn .cl.green-t { color: var(--green); }
.conn .ca { color: var(--ink-dim); font-size: 18px; line-height: 1; }
.conn.flow-anim .ca { color: var(--accent); animation: nudge 1.4s var(--ease) infinite; }
@keyframes nudge { 0%,100% { transform: translateX(0); opacity: .7; } 50% { transform: translateX(3px); opacity: 1; } }

/* cert anatomy */
.cert-wrap { display: grid; grid-template-columns: auto 1fr auto; gap: 20px; align-items: center; padding: 10px 0; }
@media (max-width: 760px) { .cert-wrap { grid-template-columns: 1fr; } }
.cert-left { text-align: center; }
.cert-left .signs { font-family: var(--font-mono); font-size: 10px; color: var(--ink-faint); margin-top: 8px; }
.cert-card { background: linear-gradient(180deg, var(--panel), var(--panel-2)); border: 1.5px solid color-mix(in srgb, var(--accent) 50%, var(--line)); border-radius: 12px; padding: 14px 16px; }
.cc-title { font-weight: 600; font-size: 13px; margin-bottom: 10px; }
.cc-row { display: grid; grid-template-columns: 110px 1fr; gap: 2px 12px; padding: 5px 0; border-top: 1px solid var(--line-soft); }
.cc-row .cc-k { font-family: var(--font-mono); font-size: 9.5px; color: var(--ink-faint); letter-spacing: .04em; text-transform: uppercase; align-self: center; }
.cc-row .cc-v { font-family: var(--font-mono); font-size: 12px; color: var(--ink); word-break: break-all; }
.cc-row .cc-a { grid-column: 2; font-size: 10.5px; color: var(--ink-faint); font-style: italic; }
.cc-row.sig .cc-v { color: var(--green); }
.cert-mtls { text-align: center; }
.mt-pair { display: flex; align-items: center; gap: 8px; justify-content: center; }
.mt-arrows { color: var(--green); font-size: 18px; }
.mt-cap { font-size: 11px; color: var(--ink-dim); margin-top: 8px; line-height: 1.4; max-width: 150px; }
.mt-cap b { color: var(--accent); }

/* spire ttl ring */
.ttl { display: flex; flex-direction: column; align-items: center; min-width: 90px; }
.ttl-ring { width: 52px; height: 52px; border-radius: 50%; border: 4px solid var(--line); position: relative; overflow: hidden; }
.ttl-ring .ttl-fill { position: absolute; inset: -4px; border-radius: 50%; border: 4px solid var(--accent);
  clip-path: inset(0 0 0 0); animation: ttl 4s linear infinite; }
@keyframes ttl { 0% { transform: rotate(0deg); opacity: 1; } 85% { opacity: 1; } 92% { opacity: .2; border-color: var(--green); } 100% { transform: rotate(360deg); opacity: 1; border-color: var(--accent); } }
.ttl-cap { font-size: 10.5px; color: var(--ink-dim); margin-top: 8px; text-align: center; line-height: 1.4; }
.ttl-cap b { color: var(--accent); }

/* ory jwt card */
.jwt { background: linear-gradient(180deg, var(--panel), var(--panel-2)); border: 1.5px solid color-mix(in srgb, var(--amber) 50%, var(--line)); border-radius: 10px; padding: 10px 14px; }
.jwt-t { font-weight: 600; font-size: 12px; margin-bottom: 6px; }
.jwt-r { display: flex; gap: 14px; font-family: var(--font-mono); font-size: 11px; }
.jwt-r span:first-child { color: var(--ink-faint); width: 48px; }
.jwt-r span:last-child { color: var(--ink); }

/* acl pdp/pep */
.acl-wrap { padding: 8px 0; }
.acl-top { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; justify-content: center; }
.pep { display: flex; align-items: center; gap: 8px; }
.pep .pep-bar { width: 4px; height: 70px; background: var(--ink-dim); border-radius: 2px; }
.pep .pep-l { font-family: var(--font-mono); font-size: 10px; color: var(--ink-dim); }
.pep .pep-l span { color: var(--ink-faint); }
.acl-branch { display: flex; flex-direction: column; gap: 10px; }
.branch { display: flex; align-items: center; gap: 8px; }
.branch .ca { font-size: 18px; }
.branch.ok .ca { color: var(--green); }
.branch.no .ca { color: var(--pink); }
.acl-pdp { display: flex; flex-direction: column; align-items: center; margin-top: 10px; }
.acl-pdp .updash { width: 0; height: 18px; border-left: 1.5px dashed var(--accent); }
.acl-pdp .node b { color: var(--accent); }

.anat-note { margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--line-soft); color: var(--ink-dim); font-size: 13px; line-height: 1.55; text-align: center; }
.anat-note b { color: var(--ink); }
</style>
