// Same-origin client for the orchestrator (/api, /ws) and Hydra (/hydra).
// Everything the browser shows is fed by the real orchestrator — it never
// fabricates a verdict or a sample.

export type GateState = 'pass' | 'fail' | 'na'
export interface GateVerdict { gate: string; state: GateState; role: string; detail: string; live: boolean }
export interface Identity {
  handle: string; kind: string; anchor: string | null; role: string | null;
  uri_san: string | null; revoked: boolean; blurb: string
}
export interface AttemptResp {
  identity: string; condition: string; key: string; gates: GateVerdict[];
  expected: string; observed: string; silently_denied: boolean; summary: string
}
export interface MintResp {
  ok: boolean; anchor: string; rotated: boolean; cn: string; uri_san: string;
  serial: string; not_after: string; message: string
}
export interface Health { zenoh: boolean; spire: boolean; hydra: boolean; at: number }
export interface Svid { ok: boolean; spiffe_id?: string; cn?: string; serial?: string; not_after_unix?: number; ttl_seconds?: number; error?: string }

async function j<T>(r: Response): Promise<T> {
  const t = await r.text()
  try { return JSON.parse(t) as T } catch { throw new Error(t || r.statusText) }
}

export const api = {
  roster: () => fetch('/api/roster').then(j<Identity[]>),
  config: () => fetch('/api/config').then(j<any>),
  health: () => fetch('/api/health').then(j<Health>),
  svid: () => fetch('/api/spire/svid').then(j<Svid>),
  attempt: (body: any) => fetch('/api/attempt', post(body)).then(j<AttemptResp>),
  mint: (body: any) => fetch('/api/mint', post(body)).then(j<MintResp>),
  revoke: (body: any) => fetch('/api/revoke', post(body)).then(j<any>),
  resolve: () => fetch('/api/dns/resolve', post({})).then(j<any>),
  burn: (bad: boolean) => fetch('/api/dns/burn', post({ bad })).then(j<any>),
}

function post(body: any): RequestInit {
  return { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) }
}

export function connectWs(onEvent: (e: any) => void): () => void {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  let ws: WebSocket | null = null
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
function b64url(buf: ArrayBuffer): string {
  return btoa(String.fromCharCode(...new Uint8Array(buf)))
    .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}
function randStr(n: number): string {
  const a = new Uint8Array(n); crypto.getRandomValues(a)
  return Array.from(a, (x) => ('0' + (x & 0xff).toString(16)).slice(-2)).join('')
}
export async function startLogin(): Promise<void> {
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
export function getToken(): string | null { return sessionStorage.getItem('ts_id_token') }
export function logout(): void { sessionStorage.removeItem('ts_id_token') }
export function decodeJwt(token: string): any {
  try {
    const p = token.split('.')[1].replace(/-/g, '+').replace(/_/g, '/')
    return JSON.parse(decodeURIComponent(escape(atob(p))))
  } catch { return null }
}
