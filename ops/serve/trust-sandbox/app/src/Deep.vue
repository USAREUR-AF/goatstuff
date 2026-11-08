<script setup lang="ts">
// The THIRD layer: a deeper drill-down into one mechanism's anatomy, given its
// own room so the concept page's inline diagram can stay clean. Topic-driven
// (?topic=jwt | netbird), extensible to more mechanisms. Each topic also calls
// out what is an open industry standard vs the few goat-particular conventions.
import { ref } from 'vue'
import GlossaryPopup from './components/GlossaryPopup.vue'

const theme = ref(document.documentElement.getAttribute('data-theme') || 'dark')
function toggleTheme() {
  theme.value = theme.value === 'dark' ? 'light' : 'dark'
  document.documentElement.setAttribute('data-theme', theme.value)
  localStorage.setItem('tsbx-theme', theme.value)
}
const topic = new URLSearchParams(location.search).get('topic') || 'jwt'
const BACK: Record<string, { href: string; label: string }> = {
  jwt: { href: '/concept.html?gate=ory', label: 'Human identity (Ory)' },
  netbird: { href: '/concept.html?gate=netbird', label: 'Network admission (NetBird)' },
  cert: { href: '/concept.html?gate=cert', label: 'Machine identity (x509)' },
  svid: { href: '/concept.html?gate=spire', label: 'Workload identity (SPIRE)' },
  acl: { href: '/concept.html?gate=acl', label: 'The access decision (Zenoh ACL)' },
  rotate: { href: '/', label: 'the playground' },
  revoke: { href: '/', label: 'the playground' },
}
const back = BACK[topic] || BACK.jwt
</script>

<template>
  <div class="wrap">
    <header class="topbar">
      <div class="brand">
        <div class="mark">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <path d="M12 3l7 3v5c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z" /><path d="M9 12l2 2 4-4" />
          </svg>
        </div>
        <div><h1>MECHANISM</h1><p class="sub">One mechanism's anatomy, unpacked.</p></div>
      </div>
      <span class="spacer" />
      <a class="navlink" :href="back.href">◀ {{ back.label }}</a>
      <a class="navlink" href="/glossary.html">📖 GLOSSARY</a>
      <button class="theme-toggle" @click="toggleTheme"><span v-if="theme==='dark'">◐ LIGHT</span><span v-else>◑ DARK</span></button>
    </header>

    <!-- ===================== JWT / OIDC token ===================== -->
    <template v-if="topic === 'jwt'">
      <section class="chero">
        <div class="cn">JWT</div>
        <div><span class="section-label">OIDC · the token Ory issues</span>
          <h2>The OIDC ID token, taken apart</h2>
          <p class="q">What is the signed token a login actually returns?</p></div>
      </section>
      <p class="lede">When a human signs in, Ory's Hydra returns an <b>OIDC ID token</b> — a <b>JSON Web Token (JWT)</b>.
        It's just text a web service can verify and read: it says <i>who</i> you are and <i>what role</i> you hold. Here
        it is, piece by piece.</p>

      <div class="panel">
        <div class="panel-head"><span class="dot accent" /><h3>It's three parts, dot-separated</h3></div>
        <div class="jwt-seg"><span class="s-h">header</span><span class="dotsep">.</span><span class="s-p">payload</span><span class="dotsep">.</span><span class="s-s">signature</span></div>
        <p class="pp">Each part is <span class="mono">base64url</span>-encoded (encoding, not encryption — anyone can read
          it; the signature is what makes it trustworthy). <span class="std">open standard — RFC 7519</span></p>
      </div>

      <div class="grid2">
        <div class="panel">
          <div class="panel-head"><span class="dot accent" /><h3>① Header</h3></div>
          <pre class="code">{ "alg": "RS256", "typ": "JWT" }</pre>
          <p class="pp"><b>alg</b> names the signature algorithm — here <b>RS256</b> (RSA signature over SHA-256). It tells
            the verifier how to check the signature. <span class="std">open standard</span></p>
        </div>
        <div class="panel">
          <div class="panel-head"><span class="dot ask" /><h3>③ Signature</h3></div>
          <p class="pp">Hydra signs <span class="mono">base64(header) + "." + base64(payload)</span> with its <b>private
            key</b>. A service verifies it with Hydra's <b>public key</b>, fetched from the <b>JWKS</b> endpoint
            (<span class="mono">/.well-known/jwks.json</span>). Tamper with any byte → the signature fails.
            <span class="std">open standard — JWS / JWKS</span></p>
        </div>
      </div>

      <div class="panel">
        <div class="panel-head"><span class="dot amber" /><h3>② Payload — the claims</h3></div>
        <table class="kv">
          <tr><td class="k mono">iss</td><td>the issuer — the Ory/Hydra URL that minted it <span class="std">standard</span></td></tr>
          <tr><td class="k mono">sub</td><td>the subject — <i>which</i> human (a stable id) <span class="std">standard</span></td></tr>
          <tr><td class="k mono">aud</td><td>audience — which client the token is for <span class="std">standard</span></td></tr>
          <tr><td class="k mono">iat / exp</td><td>issued-at / expiry — short-lived, so a stolen token lapses on its own <span class="std">standard</span></td></tr>
          <tr><td class="k mono">groups</td><td>the <b>role</b> claim (e.g. <span class="mono">[ OPERATOR ]</span>) — what the human may do <span class="ours">our convention</span></td></tr>
        </table>
        <p class="pp">A service reads <span class="mono">groups</span> and decides: most people <b>view + operate</b>; an
          <b>ADMIN</b> can additionally authorize admin actions. Ory authenticates the human + role — it does <b>not</b>
          issue machine or workload identities.</p>
      </div>

      <div class="panel">
        <div class="panel-head"><span class="dot accent" /><h3>How you get one — the OIDC flow</h3></div>
        <ol class="steps">
          <li><b>Authorization request.</b> The app sends the browser to Hydra's <span class="mono">/oauth2/auth</span> with a one-time <b>PKCE</b> challenge.</li>
          <li><b>Login.</b> Hydra hands off to <b>Kratos</b>, which checks the password / MFA.</li>
          <li><b>Consent + code.</b> Hydra redirects back with a short-lived <b>authorization code</b>.</li>
          <li><b>Token exchange.</b> The app calls <span class="mono">/oauth2/token</span> with the code + the <b>PKCE verifier</b>; Hydra returns the <b>ID token (the JWT above)</b>.</li>
        </ol>
        <p class="pp"><span class="std">all open standards — OAuth 2.0 (RFC 6749), OIDC Core, PKCE (RFC 7636)</span></p>
      </div>

      <div class="panel std-panel">
        <div class="panel-head"><span class="dot green" /><h3>Standard vs. ours</h3></div>
        <p class="pp">Everything here is plain, modern, open identity: <b>OAuth 2.0 / OpenID Connect</b>, <b>JWT/JWS</b>,
          <b>PKCE</b>, <b>JWKS</b> — the same stack industry uses. The only goat-particular convention is putting the role
          in the <span class="mono">groups</span> claim, so any standard OIDC client interoperates with us out of the box.</p>
      </div>
    </template>

    <!-- ===================== NetBird admission ===================== -->
    <template v-else-if="topic === 'netbird'">
      <section class="chero">
        <div class="cn">N</div>
        <div><span class="section-label">NetBird / WireGuard · network admission</span>
          <h2>NetBird admission, taken apart</h2>
          <p class="q">What does enrollment actually create, and how does the tunnel come up?</p></div>
      </section>
      <p class="lede">NetBird builds an encrypted peer-to-peer mesh on <b>WireGuard</b>. Enrollment turns a one-time setup
        key into a <b>local identity</b> (a keypair + config) the client uses to bring up tunnels. Admission gets your
        packets onto the network — it is <b>not</b> trust.</p>

      <div class="panel">
        <div class="panel-head"><span class="dot accent" /><h3>The keypair — Curve25519</h3></div>
        <table class="kv">
          <tr><td class="k">algorithm</td><td>WireGuard keys are <b>Curve25519</b> (the <b>X25519</b> Diffie–Hellman function) <span class="std">open standard</span></td></tr>
          <tr><td class="k">private key</td><td>32 random bytes (curve-clamped), <b>generated on the device</b> — it <b>never leaves the machine</b></td></tr>
          <tr><td class="k">public key</td><td>derived from the private key; sent to Management so peers can find + verify you</td></tr>
        </table>
        <p class="pp">The same key math secures the tunnel: WireGuard's <b>Noise</b> handshake uses Curve25519 for key
          agreement, <b>ChaCha20-Poly1305</b> to encrypt, and <b>BLAKE2s</b> to hash. <span class="std">all open standards</span></p>
      </div>

      <div class="panel">
        <div class="panel-head"><span class="dot amber" /><h3>Enrollment — what the setup key buys</h3></div>
        <ol class="steps">
          <li><b>Setup key.</b> A one-time enrollment token, issued by an admin.</li>
          <li><b>Register.</b> The device generates its keypair locally and sends its <b>public</b> key + the setup key to <b>Management</b>, which validates it and registers the peer.</li>
          <li><b>Push config.</b> Management assigns a mesh IP and pushes the allowed peers / routes.</li>
          <li><b>Up.</b> The <b>netbird client</b> (daemon) loads the local profile and brings up the WireGuard tunnels.</li>
        </ol>
      </div>

      <div class="grid2">
        <div class="panel">
          <div class="panel-head"><span class="dot ask" /><h3>The local profile</h3></div>
          <table class="kv">
            <tr><td class="k">private key</td><td>on-device only</td></tr>
            <tr><td class="k">public key</td><td>shared via Management</td></tr>
            <tr><td class="k">mesh IP</td><td>100.x, assigned</td></tr>
            <tr><td class="k">peers + routes</td><td>who you may reach</td></tr>
            <tr><td class="k mono">~/.netbird</td><td>config + state on disk, read by the daemon</td></tr>
          </table>
        </div>
        <div class="panel">
          <div class="panel-head"><span class="dot accent" /><h3>Reaching peers</h3></div>
          <p class="pp">The client tries a <b>direct</b> peer-to-peer path first (NAT traversal via <b>ICE / STUN</b>); if
            that fails it falls back to a <b>relay</b>. Either way the traffic is end-to-end WireGuard-encrypted.
            <span class="std">open standards — ICE / STUN</span></p>
          <p class="pp">Revocation is <b>mesh-first</b>: cut the peer at Management and its packets stop — no certificate
            revocation list needed.</p>
        </div>
      </div>

      <div class="panel std-panel">
        <div class="panel-head"><span class="dot green" /><h3>Standard vs. ours</h3></div>
        <p class="pp">This is all standard, open networking: <b>WireGuard</b> (Curve25519 / ChaCha20-Poly1305 / BLAKE2s,
          the Noise protocol) and <b>NetBird</b> on top of it, with <b>ICE/STUN</b> for traversal. Nothing here is
          goat-particular — we use the mesh as-is. The only opinion is the zero-trust one: <b>being on the network is not
          trust</b>; every action is still re-checked by the identity + access layers.</p>
      </div>
    </template>

    <!-- ===================== x509 cert + mTLS ===================== -->
    <template v-else-if="topic === 'cert'">
      <section class="chero">
        <div class="cn">M</div>
        <div><span class="section-label">x509 · mTLS · machine identity</span>
          <h2>The certificate + mTLS, taken apart</h2>
          <p class="q">What's in the cert, how is it made, and what secures the connection?</p></div>
      </section>
      <p class="lede">A machine proves who it is with an <b>X.509 certificate</b> — a signed statement of identity —
        presented over <b>mutual TLS</b>, where both sides show ID. Here's the cert, how it's generated, and the exact
        crypto on the wire.</p>

      <div class="panel">
        <div class="panel-head"><span class="dot accent" /><h3>The certificate</h3></div>
        <table class="kv">
          <tr><td class="k">key algorithm</td><td><b>ECDSA P-256</b> (NIST secp256r1) keypair <span class="std">open standard</span></td></tr>
          <tr><td class="k">signature</td><td><b>ECDSA-with-SHA-256</b> — the CA signs the leaf <span class="std">standard</span></td></tr>
          <tr><td class="k mono">CN</td><td>the Common Name — the access anchor the ACL keys on</td></tr>
          <tr><td class="k">URI-SAN</td><td><span class="mono">urn:goat:__goatnet__:org=…,role=…</span> — the identity grammar <span class="ours">our convention</span></td></tr>
          <tr><td class="k">issuer</td><td>the goat CA — who vouched for it</td></tr>
          <tr><td class="k">validity</td><td>short-lived; rotation is a re-key, the cut is mesh-first</td></tr>
        </table>
      </div>

      <div class="grid2">
        <div class="panel">
          <div class="panel-head"><span class="dot amber" /><h3>How it's generated</h3></div>
          <ol class="steps">
            <li>An <b>ECDSA P-256</b> keypair is generated — the private key stays with the holder.</li>
            <li>The CA fills the leaf (CN + URI-SAN + validity + public key) and <b>signs it</b> (ECDSA-SHA-256), chaining to the trusted root.</li>
            <li>No CRL/OCSP — rotation re-keys; revocation is cut at the mesh.</li>
          </ol>
        </div>
        <div class="panel">
          <div class="panel-head"><span class="dot ask" /><h3>mTLS on the wire</h3></div>
          <table class="kv">
            <tr><td class="k">protocol</td><td><b>TLS 1.3</b> (Zenoh uses rustls) <span class="std">standard</span></td></tr>
            <tr><td class="k">key exchange</td><td><b>ECDHE</b> — ephemeral, gives <b>forward secrecy</b></td></tr>
            <tr><td class="k">data cipher</td><td><b>AEAD</b>: AES-256-GCM or ChaCha20-Poly1305</td></tr>
            <tr><td class="k">mutual</td><td><b>both</b> sides present a chain-verifiable cert — neither trusted by address</td></tr>
          </table>
        </div>
      </div>

      <div class="panel std-panel">
        <div class="panel-head"><span class="dot green" /><h3>Standard vs. ours</h3></div>
        <p class="pp"><b>X.509</b>, <b>ECDSA P-256</b>, <b>TLS 1.3</b>, <b>mTLS</b> are standard, modern crypto used as-is.
          The only goat-particular piece is the <b>URI-SAN identity grammar</b> (<span class="mono">urn:goat:…</span>) —
          a convention carried inside a standard SAN field, so any X.509 / TLS tooling still works with us.</p>
      </div>
    </template>

    <!-- ===================== SPIRE SVID ===================== -->
    <template v-else-if="topic === 'svid'">
      <section class="chero">
        <div class="cn">W</div>
        <div><span class="section-label">SPIFFE / SPIRE · workload identity</span>
          <h2>The SPIRE SVID, taken apart</h2>
          <p class="q">What's in the workload's auto-rotating identity, and how is it minted?</p></div>
      </section>
      <p class="lede">A workload we run gets an <b>SVID</b> — a SPIFFE Verifiable Identity Document — from SPIRE. It's an
        X.509 cert earned by <b>attestation</b> (no password, no copied key), short-lived, and rotated continuously.</p>

      <div class="panel">
        <div class="panel-head"><span class="dot accent" /><h3>The X509-SVID</h3></div>
        <table class="kv">
          <tr><td class="k">format</td><td>an <b>X.509</b> certificate (an "X509-SVID") <span class="std">SPIFFE standard</span></td></tr>
          <tr><td class="k">key algorithm</td><td><b>ECDSA P-256</b> (SPIRE's default) <span class="std">standard</span></td></tr>
          <tr><td class="k">SPIFFE ID</td><td><span class="mono">spiffe://&lt;trust-domain&gt;/workload/…</span> — in the URI-SAN</td></tr>
          <tr><td class="k mono">CN</td><td>the access anchor — composes with the Zenoh ACL <span class="ours">our convention</span></td></tr>
          <tr><td class="k">issuer</td><td>the SPIRE server CA (ECDSA-SHA-256)</td></tr>
          <tr><td class="k">TTL</td><td>~60s · auto-rotates — a leaked one is useless in minutes</td></tr>
        </table>
      </div>

      <div class="grid2">
        <div class="panel">
          <div class="panel-head"><span class="dot amber" /><h3>How it's minted — attestation</h3></div>
          <ol class="steps">
            <li><b>Node attestation.</b> The SPIRE server verifies the node the agent runs on.</li>
            <li><b>Workload attestation.</b> The agent verifies the workload (e.g. by uid / path) against a registration entry.</li>
            <li><b>Issue + rotate.</b> The agent generates the keypair, the server signs the SVID, and the agent re-fetches before expiry — <b>no human in the loop</b>.</li>
          </ol>
        </div>
        <div class="panel">
          <div class="panel-head"><span class="dot ask" /><h3>On the wire</h3></div>
          <p class="pp">Same as the cert path: the SVID is presented over <b>mTLS (TLS 1.3, ECDHE, AEAD)</b>, and the
            access decision matches the SVID's CN to its anchor. SPIRE is the other <i>issuer</i> — same destination as
            the CA path.</p>
        </div>
      </div>

      <div class="panel">
        <div class="panel-head"><span class="dot accent" /><h3>Where SPIRE's authority comes from</h3></div>
        <p class="pp">SPIRE does <b>not</b> borrow the goat CA. In goatnet the <b>SPIRE server is its own CA</b> — it
          holds its own signing key and is an <b>independent trust root</b>, co-located on the site's edge node, and it
          signs every SVID itself. (SPIRE <i>can</i> be chained under an <b>upstream</b> CA via an UpstreamAuthority
          plugin — e.g. Vault or another SPIRE — but goatnet runs it as a standalone root.)</p>
        <p class="pp">So goatnet has <b>two issuers with two roots</b>: the offline goat CA (org / machine certs) and
          SPIRE (the workloads we run the agent for). The fabric trusts <b>both</b> — the Zenoh router's client trust
          bundle is the <b>union</b> (<span class="mono">goat-CA-root ⊕ SPIRE-root</span>), so a cert from either issuer
          is accepted, and the access decision then matches the <b>CN</b> regardless of which issuer minted it.</p>
      </div>

      <div class="panel std-panel">
        <div class="panel-head"><span class="dot green" /><h3>Standard vs. ours</h3></div>
        <p class="pp"><b>SPIFFE / SPIRE</b> and the <b>X509-SVID</b> are open standards (CNCF). The only goat-particular
          piece is the CN=anchor convention that lets the SVID compose with our Zenoh access decision.</p>
      </div>

      <div class="panel now-soon">
        <div class="panel-head"><span class="dot accent" /><h3>Where this is today</h3></div>
        <p class="pp"><span class="badge-now">LIVE</span> One internal SPIRE trust domain on the goatnet edge node,
          issuing auto-rotated CN-bearing SVIDs to workloads we run the agent for (the fabric-health meter is the first
          live consumer on __GOATNET__).</p>
        <p class="pp"><span class="badge-soon">DESIGNED</span> A mission partner runs its <b>own</b> SPIRE inside its
          pod — its own trust domain — and <b>federates</b>, exchanging only public trust bundles. The pod then issues
          and rotates its workloads <b>locally</b> and never hits the goat core for certs; each side's SPIRE is a SPOF
          only for its own domain. This federation is designed-not-built — it lands with the moon-pod SPIRE overlay
          (__goatnet__-moon-pod).</p>
      </div>
    </template>

    <template v-else-if="topic === 'acl'">
      <section class="chero">
        <div class="cn">D</div>
        <div><span class="section-label">Zenoh ACL · the access decision</span>
          <h2>The access decision, taken apart</h2>
          <p class="q">When a peer writes to the fabric, what decides yes/no — against what, and who enforces it?</p></div>
      </section>
      <p class="lede">Identity (the cert or SVID) only says <i>who</i> you are. The <b>access decision</b> is a separate
        step that says whether <i>that</i> identity may perform <i>this</i> action. In goatnet it's a static ACL the
        Zenoh router evaluates on every write.</p>

      <div class="panel">
        <div class="panel-head"><span class="dot accent" /><h3>What enforces it</h3></div>
        <p class="pp">The <b>Zenoh router itself</b> (<span class="mono">zenohd</span>) — not a separate gatekeeper
          service. The same router that routes the data evaluates its <span class="mono">access_control</span> policy at
          <b>ingress</b>, on every publication. Because the fabric is <b>brokerless</b>, there's no central
          policy-enforcement point: each router enforces the same rendered ACL independently.</p>
      </div>

      <div class="panel">
        <div class="panel-head"><span class="dot amber" /><h3>What it matches against</h3></div>
        <p class="pp">A grant is three things — <b>subject × action × resource</b>:</p>
        <table class="kv">
          <tr><td class="k">subject</td><td>the <b>verified cert / SVID CN</b> — the identity the mTLS handshake already
            proved. (Not an IP, not a self-claimed name.)</td></tr>
          <tr><td class="k">action</td><td><b>put / write.</b> Reads are mostly permissive; the ACL is the
            <b>write-integrity</b> layer.</td></tr>
          <tr><td class="k">resource</td><td>a <b>key-expression</b> — the topic namespace the identity owns
            (its <b>anchor</b>, e.g. <span class="mono">&lt;anchor&gt;/**</span>).</td></tr>
        </table>
        <p class="pp">So the decision is: <i>does a rule grant <b>this CN</b> the <b>put</b> action on <b>this
          key-expression</b>?</i> The CN is bound to the namespace at onboarding, which is why an identity can only write
          under its own anchor.</p>
      </div>

      <div class="grid2">
        <div class="panel">
          <div class="panel-head"><span class="dot pink" /><h3>Default-deny — and silent</h3></div>
          <p class="pp">No matching grant → the write is <b>dropped at ingress</b>. Critically, the sender gets
            <b>no error</b> — the publish call succeeds locally but the sample never lands. That silent denial is a real
            property of the fabric (you confirm a write by observing it land, not by a return code), and it's exactly
            what the playground's "wrong lane" knockout demonstrates.</p>
        </div>
        <div class="panel">
          <div class="panel-head"><span class="dot ask" /><h3>Static, not per-request</h3></div>
          <p class="pp">The ACL is <b>rendered from the registry</b> ahead of time, not computed per request. It answers
            "may this identity write here," not "is this identity still allowed to exist." Cutting an identity off
            (revocation) happens one layer out — at <b>NetBird</b> (mesh admission) — so the ACL stays simple and the
            router never has to phone home.</p>
        </div>
      </div>

      <div class="panel std-panel">
        <div class="panel-head"><span class="dot green" /><h3>Standard vs. ours</h3></div>
        <p class="pp">Binding authorization to the <b>verified cert CN</b>, default-deny, and enforcing at the data
          plane are ordinary, sound patterns. The goat-particular pieces are the <b>pre-allocated slot-pool anchor
          namespace</b> (so a CN maps cleanly to a write namespace) and putting the <b>dynamic cut-off at the mesh</b>
          rather than in a per-request authorization service — a deliberate consequence of staying brokerless.</p>
      </div>
    </template>

    <template v-else-if="topic === 'rotate'">
      <section class="chero">
        <div class="cn">↻</div>
        <div><span class="section-label">Lifecycle · rotation</span>
          <h2>Rotation — a fresh key for the same identity</h2>
          <p class="q">How do you re-key a peer without re-onboarding it or breaking its access?</p></div>
      </section>
      <p class="lede">Rotation re-issues a <b>fresh leaf certificate for the same identity</b>. The anchor, CN,
        write-namespace, alias, and ACL all stay exactly the same — only the leaf cert (and its portal API key) are
        new. It's the onboarding mint <i>minus</i> the slot allocation.</p>

      <div class="grid2">
        <div class="panel">
          <div class="panel-head"><span class="dot amber" /><h3>What changes — and what doesn't</h3></div>
          <table class="kv">
            <tr><td class="k">changes</td><td>the <b>leaf cert</b> (new keypair + signature) + the portal <b>api_key</b></td></tr>
            <tr><td class="k">unchanged</td><td><b>anchor / CN</b>, URI-SAN, slot id, alias, write-namespace, <b>ACL rule</b></td></tr>
            <tr><td class="k">cost</td><td><b>no ACL render, no router restart</b> — the rule keys on the unchanged CN. Zero slots consumed.</td></tr>
          </table>
        </div>
        <div class="panel">
          <div class="panel-head"><span class="dot ask" /><h3>Why you'd rotate</h3></div>
          <ol class="steps">
            <li><b>Expiry renewal</b> — replace a leaf before it ages out.</li>
            <li><b>Post-compromise re-key</b> — a key may have leaked; issue a fresh one (compose with a mesh-first cut, below).</li>
            <li><b>Scheme retrofit</b> — re-derive the URI-SAN from the current minter config onto an already-issued cert.</li>
          </ol>
        </div>
      </div>

      <div class="panel">
        <div class="panel-head"><span class="dot pink" /><h3>The catch — rotation is a re-key, not a revoke</h3></div>
        <p class="pp">Rotation does <b>not</b> cryptographically invalidate the old leaf. goat has <b>no CRL/OCSP</b> by
          design, so the old cert keeps working until it expires. If a leaked key must die <i>now</i>, compose rotation
          with a <a class="xlink" href="/deep.html?topic=revoke">mesh-first cut</a> — re-key the identity, then remove
          the old peer's mesh path so the old leaf can't reach the fabric.</p>
      </div>

      <div class="panel">
        <div class="panel-head"><span class="dot accent" /><h3>SPIRE rotates itself</h3></div>
        <p class="pp">For SPIRE workloads, rotation is <b>automatic and continuous</b> — the agent re-fetches a fresh
          SVID before expiry (~every 15 min), no operator action. Same idea (fresh key, same identity), different
          trigger: operator-driven for certs, self-driving for SVIDs. Short TTLs mean a leaked SVID is useless in
          minutes — which is also why SPIRE doesn't need a CRL.</p>
      </div>

      <div class="panel std-panel">
        <div class="panel-head"><span class="dot green" /><h3>Standard vs. ours</h3></div>
        <p class="pp">Re-keying short-lived certs and continuous SVID rotation are ordinary modern PKI hygiene. The
          goat-particular piece is <b>in-place re-issue against the same slot anchor</b> — so a rotation never churns
          the slot pool, never re-renders the ACL, and never restarts the router.</p>
      </div>

      <div class="panel now-soon">
        <div class="panel-head"><span class="dot accent" /><h3>Where this is today</h3></div>
        <p class="pp"><span class="badge-now">LIVE</span> Operator-driven rotation (portal
          <span class="mono">POST /api/vendors/&lt;anchor&gt;/rotate</span> + <span class="mono">goat admin rotate</span>)
          and SPIRE auto-rotation are deployed and live-fired on __GOATNET__ today.</p>
        <p class="pp"><span class="badge-soon">DESIGNED</span> A <b>federated</b> partner pod rotates its <i>own</i>
          workloads locally (it runs its own SPIRE) — designed-not-built; it lands with the moon-pod SPIRE overlay
          (__goatnet__-moon-pod).</p>
      </div>
    </template>

    <template v-else-if="topic === 'revoke'">
      <section class="chero">
        <div class="cn">⊘</div>
        <div><span class="section-label">Lifecycle · revocation</span>
          <h2>Revocation — mesh-first, because there's no CRL</h2>
          <p class="q">How do you take a principal off the fabric when you can't revoke a cert cryptographically?</p></div>
      </section>
      <p class="lede">goat issues <b>no CRL or OCSP</b> — by design. So revocation doesn't try to invalidate a cert
        cryptographically. It cuts the principal off at the layer that actually controls reach: <b>the mesh</b>. A cert
        that can't get onto the network can't do anything.</p>

      <div class="panel">
        <div class="panel-head"><span class="dot accent" /><h3>Mesh-first — the authoritative cut</h3></div>
        <p class="pp">The cut-off lives at <b>NetBird</b> (machines) and <b>Ory</b> (humans / services), <b>not</b> at
          the Zenoh ACL and <b>not</b> via CRL. Because the fabric is <b>dark-by-default / mesh-only</b>, removing a
          principal's mesh peers removes all reach — the cert is inert without a mesh path. Stripping the ACL grant is
          defense-in-depth on top, not the primary mechanism.</p>
      </div>

      <div class="grid2">
        <div class="panel">
          <div class="panel-head"><span class="dot amber" /><h3>Enumerate, then cut</h3></div>
          <p class="pp">A principal can hold <b>many peers, many setup keys, many certs</b>. So revocation is
            <b>inventory-driven enumerate-and-cut</b>: discover every artifact bound to the principal (from the issuance
            registry + the peer-name prefix), then delete each at its authoritative layer. The hard part is enumeration
            <i>completeness</i> — a peer enrolled out-of-band can be missed (backstopped by dark-by-default + a
            reconciliation probe).</p>
        </div>
        <div class="panel">
          <div class="panel-head"><span class="dot ask" /><h3>Principal-scoped + reversible</h3></div>
          <ol class="steps">
            <li><b>Scoped.</b> Person by default; <span class="mono">org</span> scope cascades to all members (persons
              and systems are siblings), following <b>containment</b>, not custody.</li>
            <li><b>Reversible by default.</b> Ory identities are <i>deactivated</i>, not deleted
              (<span class="mono">--purge</span> for hard delete). Idempotent, and every cut emits a <b>revoke receipt
              + audit row</b>.</li>
          </ol>
        </div>
      </div>

      <div class="panel">
        <div class="panel-head"><span class="dot pink" /><h3>Why short-lived certs make this safe</h3></div>
        <p class="pp">Without a CRL, the residual risk is the window between the cut and the cert's natural expiry. goat
          keeps that window small with <b>short certificate lifetimes</b> — and for SPIRE workloads, revocation is
          simply <b>removing the registration entry</b>: no new SVID is issued and the existing one expires in ~15
          minutes. Short TTLs are the substitute for a revocation list.</p>
      </div>

      <div class="panel std-panel">
        <div class="panel-head"><span class="dot green" /><h3>Standard vs. ours</h3></div>
        <p class="pp">Short-lived certs + no-CRL is an increasingly standard modern pattern (SPIFFE is built on it).
          The goat-particular pieces are making the <b>mesh</b> the authoritative cut-off (because the fabric is
          dark-by-default and the ACL is static), and the <b>registry-driven enumerate-and-cut</b> across a principal's
          many artifacts.</p>
      </div>

      <div class="panel now-soon">
        <div class="panel-head"><span class="dot accent" /><h3>Where this is today</h3></div>
        <p class="pp"><span class="badge-now">LIVE</span> The mesh-first cut (NetBird / Ory peer + setup-key removal,
          ACL de-authorization) runs on __GOATNET__ today via <span class="mono">goat admin onboard &lt;handle&gt; --revoke</span>.</p>
        <p class="pp"><span class="badge-soon">PARTLY BUILT</span> The full registry-driven enumerate-and-cut depends on
          the issuance registry (staged). <span class="badge-soon">DESIGNED</span> A <b>federated</b> partner pod
          revokes its <i>own</i> workloads locally — designed-not-built; it lands with the moon-pod SPIRE overlay
          (__goatnet__-moon-pod).</p>
      </div>
    </template>

    <footer class="foot">
      <a :href="back.href">◀ back to {{ back.label }}</a>
    </footer>
    <GlossaryPopup />
  </div>
</template>

<style scoped>
.chero { display: flex; gap: 22px; align-items: center; margin: 6px 2px 14px; padding-bottom: 14px; border-bottom: var(--seam) solid var(--line-soft); }
.chero .cn { font-size: clamp(34px, 6vw, 60px); font-weight: 800; color: var(--accent); line-height: 1; opacity: .9; font-family: var(--font-mono); }
.chero h2 { font-size: clamp(20px, 3.4vw, 30px); margin: 6px 0 6px; }
.chero .q { margin: 0; color: var(--ink); font-size: 16px; font-weight: 500; }
.lede { color: var(--ink); font-size: 15px; line-height: 1.6; max-width: 80ch; margin: 0 0 18px;
  border-left: 2px solid color-mix(in srgb, var(--accent) 30%, transparent); padding-left: 12px; }
.panel { margin-bottom: 16px; }
.grid2 { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 16px; margin-bottom: 16px; }
.grid2 .panel { margin-bottom: 0; }
.pp { margin: 0; color: var(--ink-dim); font-size: 13.5px; line-height: 1.6; }
.pp + .pp { margin-top: 10px; }
.pp b { color: var(--ink); }
.code { font-family: var(--font-mono); font-size: 13px; background: var(--panel-2); border: var(--seam) solid var(--line);
  border-radius: 8px; padding: 10px 12px; margin: 0 0 10px; color: var(--accent); overflow-x: auto; }
.jwt-seg { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-family: var(--font-mono); font-weight: 700; margin-bottom: 10px; font-size: 14px; }
.jwt-seg .dotsep { color: var(--ink-faint); }
.jwt-seg .s-h { color: var(--ask); background: color-mix(in srgb, var(--ask) 16%, transparent); padding: 4px 12px; border-radius: 6px; }
.jwt-seg .s-p { color: var(--amber); background: color-mix(in srgb, var(--amber) 16%, transparent); padding: 4px 12px; border-radius: 6px; }
.jwt-seg .s-s { color: var(--green); background: color-mix(in srgb, var(--green) 16%, transparent); padding: 4px 12px; border-radius: 6px; }
.kv { width: 100%; border-collapse: collapse; font-size: 13px; }
.kv td { padding: 7px 8px; border-bottom: var(--seam) solid var(--line-soft); vertical-align: top; color: var(--ink-dim); line-height: 1.5; }
.kv .k { color: var(--ink); font-weight: 600; white-space: nowrap; width: 1%; padding-right: 16px; }
.kv b { color: var(--ink); }
.steps { margin: 0; padding-left: 20px; display: flex; flex-direction: column; gap: 9px; color: var(--ink-dim); font-size: 13.5px; line-height: 1.55; }
.steps b { color: var(--ink); }
.std, .ours { font-family: var(--font-mono); font-size: 9.5px; letter-spacing: .04em; padding: 2px 7px; border-radius: 999px; white-space: nowrap; }
.std { color: var(--green); border: 1px solid color-mix(in srgb, var(--green) 45%, var(--line)); }
.ours { color: var(--amber); border: 1px solid color-mix(in srgb, var(--amber) 45%, var(--line)); }
.std-panel { border-left: 4px solid var(--green); }
.now-soon { border-left: 4px solid var(--accent); }
.badge-now, .badge-soon { display: inline-block; font-family: var(--font-mono); font-size: 10px; font-weight: 700;
  letter-spacing: .05em; padding: 1px 6px; border-radius: 5px; margin-right: 7px; vertical-align: middle; }
.badge-now { color: var(--green); border: 1px solid color-mix(in srgb, var(--green) 50%, transparent); }
.badge-soon { color: var(--amber); border: 1px solid color-mix(in srgb, var(--amber) 50%, transparent); }
.pp a.xlink { color: var(--accent); text-decoration: none; border-bottom: 1px dotted color-mix(in srgb, var(--accent) 55%, transparent); }
.pp a.xlink:hover { border-bottom-style: solid; }
.foot { margin-top: 24px; }
</style>
