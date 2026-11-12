// SPDX-License-Identifier: Apache-2.0
// Shared glossary: term → { full name, plain-language definition }. Used by the
// auto-badging popup (gloss-popup.js) and the glossary page.
// Newcomer-first: every acronym/term the sandbox uses, defined in one line.
// Entry shape: { term, full?, def }

export const GLOSSARY = [
  { term: 'zero trust', def: 'Trust nothing by default — verify identity per action, every time. There is no trusted "inside".' },
  { term: 'NetBird', def: 'The mesh VPN (built on WireGuard) that admits a device to the network. Admission is not trust.' },
  { term: 'WireGuard', def: 'The fast, modern VPN protocol NetBird uses to build the encrypted peer-to-peer mesh.' },
  { term: 'mesh', def: 'The flat, encrypted peer-to-peer network — peers talk directly, not through a central hub.' },
  { term: 'x509', def: 'The standard format for a public-key certificate: a signed statement of identity.' },
  { term: 'CA', full: 'Certificate Authority', def: 'The trusted party that signs certificates. A cert is only as good as the CA that signed it.' },
  { term: 'CN', full: 'Common Name', def: 'The main name field in a certificate — here, the access anchor the access rules key on.' },
  { term: 'SAN', full: 'Subject Alternative Name', def: 'Extra identity fields in a certificate — here it carries the structured identity grammar.' },
  { term: 'URI-SAN', def: 'A URI placed in the certificate’s SAN — the identity grammar, e.g. urn:goat:__goatnet__:org=…,role=….' },
  { term: 'mTLS', full: 'mutual TLS', def: 'TLS where BOTH sides present a certificate and verify the other — not just the server.' },
  { term: 'TLS', full: 'Transport Layer Security', def: 'The protocol that encrypts and authenticates a network connection (the "s" in https).' },
  { term: 'SPIFFE', full: 'Secure Production Identity Framework For Everyone', def: 'An open standard for giving workloads (software) a verifiable identity.' },
  { term: 'SPIRE', full: 'the SPIFFE Runtime Environment', def: 'The runtime that implements SPIFFE — it attests workloads and issues their identities.' },
  { term: 'SVID', full: 'SPIFFE Verifiable Identity Document', def: 'A workload’s short-lived, auto-rotating identity document, issued by SPIRE.' },
  { term: 'Ory', def: 'The open-source identity stack (Kratos + Hydra) that logs in humans and carries their role.' },
  { term: 'Kratos', def: 'Ory’s identity/credential store — handles human login (passwords, MFA).' },
  { term: 'Hydra', def: 'Ory’s OAuth2 / OpenID Connect server — issues the signed token after a human logs in.' },
  { term: 'OIDC', full: 'OpenID Connect', def: 'The standard login protocol (built on OAuth2) that proves who a human is.' },
  { term: 'OAuth2', def: 'The standard for delegated authorization that OpenID Connect builds on.' },
  { term: 'JWT', full: 'JSON Web Token', def: 'A compact, signed token carrying claims (who you are, your role) — verifiable without a database.' },
  { term: 'Zenoh', def: 'The data fabric (publish/subscribe) the sandbox writes to; its access rules make the per-write decision.' },
  { term: 'ACL', full: 'Access Control List', def: 'The rules that decide which identity may do what — default-deny here.' },
  { term: 'PEP', full: 'Policy Enforcement Point', def: 'The checkpoint on the request path that enforces the access decision.' },
  { term: 'PDP', full: 'Policy Decision Point', def: 'The policy "brain" that decides allow or deny; the enforcement point applies what it says.' },
  { term: 'TTL', full: 'Time To Live', def: 'How long a credential is valid before it expires — short here, so a leaked one dies fast.' },
  { term: 'CoreDNS', def: 'The DNS server that resolves mesh names to addresses. A name carries no trust — only an address.' },
  { term: 'anchor', def: 'The namespace an identity is allowed to write under — its lane on the fabric (keyed on the cert CN).' },
  { term: 'attestation', def: 'Proving a workload is what it claims by checking the node + process — how SPIRE earns an identity.' },
  { term: 'burn & respawn', def: 'Tear a service down and stand a fresh one up under the same name; trust is re-earned per connection.' },
  { term: '__GOATNET__ Backbone', def: 'The data fabric this sandbox models — a notional, ephemeral copy of its zero-trust design, for illustration.' },
  { term: '__GOATNET__', def: 'The goatnet whose data backbone this sandbox models.' },
  { term: 'DNSSEC', full: 'DNS Security Extensions', def: 'Signs DNS answers so you can tell the name → address mapping wasn’t tampered with. Authenticates the lookup, not the endpoint.' },
  { term: 'blast radius', def: 'How far the damage spreads when something goes wrong. Zero trust’s core job is keeping it to one room.' },
  { term: 'social engineering', def: 'Tricking a person (e.g. phishing) into giving access — instead of breaking the technology. The root of most real breaches.' },
  { term: 'LUKS', full: 'Linux Unified Key Setup', def: 'Full-disk encryption at rest — if a drive is stolen, the data on it stays unreadable.' },
  { term: 'forward secrecy', def: 'Each session uses fresh ephemeral keys, so stealing a key later can’t decrypt past recorded traffic.' },
  { term: 'post-quantum', def: 'Encryption algorithms designed to resist future quantum computers — the crypto can migrate to them without redesign.' },
  { term: 'PQC', full: 'Post-Quantum Cryptography', def: 'Encryption built to resist quantum computers; the fabric’s crypto is swappable toward it.' },
  { term: 'FIPS', full: 'Federal Information Processing Standards', def: 'US government standards for validated cryptography — the crypto can run in a FIPS-validated mode where required.' },
  { term: 'defense in depth', def: 'Layering independent protections so no single failure is fatal — e.g. two separate encrypted tunnels.' },
  { term: 'Noise', def: 'The cryptographic handshake framework WireGuard uses to set up its encrypted tunnel.' },
  { term: 'end-to-end', def: 'Encrypted the whole way between the two endpoints — no point in the middle sees plaintext.' },
  // the five-aspect symbology — chip letters, looked up when a chip is clicked (not auto-badged in prose)
  { term: 'N', full: 'Network admission', def: 'Are you allowed onto the mesh at all? Admission is set once at onboarding (NetBird / WireGuard). Being on the network is not trust.' },
  { term: 'M', full: 'Machine identity (x509)', def: 'A signed x509 certificate proving what a machine is — a scoped, revocable keycard; with mTLS both sides present one.' },
  { term: 'W', full: 'Workload identity (SPIRE)', def: 'An attested, auto-rotating SPIRE / SPIFFE identity for a software workload — short-lived, no human ever touches it.' },
  { term: 'H', full: 'Human identity + role (Ory)', def: 'A human signed in via Ory, carrying their role — authenticates the actor and authorizes admin actions like a mint.' },
  { term: 'D', full: 'the access Decision (Zenoh ACL)', def: 'The per-action authorization decision, default-deny, keyed to the identity (cert / SVID) you present.' },
]

// normalized lookup
const byKey = new Map()
for (const e of GLOSSARY) byKey.set(e.term.toLowerCase(), e)
export function lookup(term) { return byKey.get(term.toLowerCase()) }

// terms ordered longest-first so multi-word / hyphenated match before substrings
export const TERMS_BY_LEN = [...GLOSSARY.map((e) => e.term)].sort((a, b) => b.length - a.length)
