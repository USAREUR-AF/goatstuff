// SPDX-License-Identifier: Apache-2.0
// Same-origin client for the orchestrator (/api, /ws) and Hydra (/hydra).
// Everything the browser shows is fed by the real orchestrator — it never
// fabricates a verdict or a sample.
//
// Shapes the orchestrator returns:
//   Identity    { handle, kind, anchor, role, uri_san, revoked, blurb }
//   GateVerdict { gate, state: 'pass'|'fail'|'na', role, detail, live }
//   AttemptResp { identity, condition, key, gates: GateVerdict[], expected, observed, silently_denied, summary }
//   MintResp    { ok, anchor, rotated, cn, uri_san, serial, not_after, message }
//   Health      { zenoh, spire, hydra, at }
//   Svid        { ok, spiffe_id?, cn?, serial?, not_after_unix?, ttl_seconds?, error? }

async function j(r) {
  const t = await r.text()
  try { return JSON.parse(t) } catch { throw new Error(t || r.statusText) }
}

function post(body) {
  return { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) }
}

export const api = {
  roster: () => fetch('/api/roster').then(j),
  config: () => fetch('/api/config').then(j),
  health: () => fetch('/api/health').then(j),
  svid: () => fetch('/api/spire/svid').then(j),
  attempt: (body) => fetch('/api/attempt', post(body)).then(j),
  mint: (body) => fetch('/api/mint', post(body)).then(j),
  revoke: (body) => fetch('/api/revoke', post(body)).then(j),
  resolve: () => fetch('/api/dns/resolve', post({})).then(j),
  burn: (bad) => fetch('/api/dns/burn', post({ bad })).then(j),
}

export function connectWs(onEvent) {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  let ws = null
  let closed = false
  const open = () => {
    ws = new WebSocket(`${proto}://${location.host}/ws`)
    ws.onmessage = (m) => { try { onEvent(JSON.parse(m.data)) } catch { /* ignore */ } }
    ws.onclose = () => { if (!closed) setTimeout(open, 1500) }
    ws.onerror = () => { try { ws?.close() } catch { /* */ } }
  }
  open()
  return () => { closed = true; try { ws?.close() } catch { /* */ } }
}

// --- Ory / OIDC (PKCE authorization-code flow against Hydra via /hydra) ------
function b64url(buf) {
  return btoa(String.fromCharCode(...new Uint8Array(buf)))
    .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}
function randStr(n) {
  const a = new Uint8Array(n); crypto.getRandomValues(a)
  return Array.from(a, (x) => ('0' + (x & 0xff).toString(16)).slice(-2)).join('')
}
export async function startLogin() {
  const verifier = randStr(48)
  const challenge = b64url(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(verifier)))
  const state = randStr(16)
  sessionStorage.setItem('ts_pkce_verifier', verifier)
  sessionStorage.setItem('ts_oauth_state', state)
  const p = new URLSearchParams({
    response_type: 'code',
    client_id: 'trust-sandbox-spa',
    scope: 'openid offline groups',
    redirect_uri: location.origin + '/callback.html',
    state,
    code_challenge: challenge,
    code_challenge_method: 'S256',
  })
  location.href = `/hydra/oauth2/auth?${p.toString()}`
}
export function getToken() { return sessionStorage.getItem('ts_id_token') }
export function logout() { sessionStorage.removeItem('ts_id_token') }
export function decodeJwt(token) {
  try {
    const p = token.split('.')[1].replace(/-/g, '+').replace(/_/g, '/')
    return JSON.parse(decodeURIComponent(escape(atob(p))))
  } catch { return null }
}
