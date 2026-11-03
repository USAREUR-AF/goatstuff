// The reactive heart of the sandbox: one Zenoh session, and the live state of
// everything the visitor has declared on it (publishers, subscribers,
// queryables) plus a friendly activity log. Built on @eclipse-zenoh/zenoh-ts;
// every method maps to one Zenoh concept so the UI can stay narrative.
//
// API notes (zenoh-ts 1.9.0, verified against upstream src):
//  - Session.open(new Config(url)) — url is a FULL ws(s):// URL incl. path;
//    the client's parseZenohLocator returns such URLs verbatim, so our
//    Traefik wss://host/ws path survives.
//  - declareSubscriber / get / declareQueryable accept a *callback* handler.
//  - sample.keyexpr().toString(), sample.payload().toString(), sample.kind().
import { reactive } from 'vue'
import {
  Config, Session, KeyExpr, Sample, SampleKind,
  type Publisher, type Subscriber, type Queryable, type Reply, type Query,
} from '@eclipse-zenoh/zenoh-ts'

export type ConnStatus = 'idle' | 'connecting' | 'connected' | 'error'

export interface FeedItem { seq: number; topic: string; payload: string; kind: string; at: number; fresh: boolean }
export interface SubState { id: number; keyExpr: string; sub: Subscriber; feed: FeedItem[]; count: number }
export interface PubState { id: number; keyExpr: string; pub: Publisher; payload: string; periodMs: number; sent: number; timer?: ReturnType<typeof setInterval> }
export interface QState { id: number; keyExpr: string; reply: string; q: Queryable; replied: number }
export interface XformState { id: number; inExpr: string; outTopic: string; label: string; sub: Subscriber; count: number; last: { in: string; out: string } | null; error: string }
export interface LogItem { id: number; at: number; tone: 'info' | 'send' | 'recv' | 'query' | 'reply' | 'xform' | 'error' | 'system'; text: string }

// A single observable moment of data moving, for the live topology viz.
// dir 'out' = node → router (a publish/reply); dir 'in' = router → node (a receive).
export type FlowNode = 'pub' | 'sub' | 'queryable' | 'xform'
export interface FlowEvent { node: FlowNode; id: number; dir: 'in' | 'out'; topic: string }

function now() { return Date.now() }

export const z = reactive({
  url: defaultUrl(),
  status: 'idle' as ConnStatus,
  error: '' as string,
  session: null as Session | null,

  subs: [] as SubState[],
  pubs: [] as PubState[],
  queryables: [] as QState[],
  xforms: [] as XformState[],
  log: [] as LogItem[],

  _seq: 0,
  _id: 0,
  _logId: 0,

  // Flow-event bus for the topology viz. Kept off the reactive path on purpose
  // (these fire many times/sec; we don't want Vue tracking them).
  _flow: [] as ((e: FlowEvent) => void)[],
  onFlow(cb: (e: FlowEvent) => void) { this._flow.push(cb) },
  emitFlow(e: FlowEvent) { for (const cb of this._flow) cb(e) },

  get connected(): boolean { return this.status === 'connected' && !!this.session },

  note(tone: LogItem['tone'], text: string) {
    this.log.unshift({ id: this._logId++, at: now(), tone, text })
    if (this.log.length > 300) this.log.length = 300
  },

  async connect() {
    if (this.status === 'connecting' || this.connected) return
    this.status = 'connecting'; this.error = ''
    this.note('system', `Reaching out to the fabric at ${this.url} …`)
    try {
      this.session = await Session.open(new Config(this.url))
      this.status = 'connected'
      this.note('system', `Connected. You're on the fabric. 🎉`)
    } catch (e: any) {
      this.status = 'error'
      this.error = humanError(e)
      this.note('error', `Couldn't connect: ${this.error}`)
    }
  },

  async disconnect() {
    for (const p of this.pubs) if (p.timer) clearInterval(p.timer)
    try { await this.session?.close() } catch { /* best-effort */ }
    this.session = null
    this.subs = []; this.pubs = []; this.queryables = []; this.xforms = []
    this.status = 'idle'
    this.note('system', 'Left the fabric. Everything you declared is gone — that’s the point of a sandbox.')
  },

  // --- Publish: put a value onto a topic -----------------------------------
  // One-shot put, no declared publisher — the quick path.
  async putOnce(keyExpr: string, payload: string) {
    if (!this.session) return
    try {
      await this.session.put(keyExpr, payload)
      // id -1 = an ad-hoc put with no declared-publisher node; the viz draws it
      // from off-stage. dir 'out' carries the concrete topic so the viz fans it
      // out to the matching consumers.
      this.emitFlow({ node: 'pub', id: -1, dir: 'out', topic: keyExpr })
      this.note('send', `put → “${keyExpr}”: ${payload}`)
    } catch (e: any) { this.note('error', `put failed: ${humanError(e)}`) }
  },

  // Declare a publisher on a topic. periodMs === 0 → manual (use publishNow);
  // periodMs > 0 → auto-publishes the payload every periodMs (the original
  // demo's "Period ms (0 = manual only)" behavior).
  async declarePublisher(keyExpr: string, payload: string, periodMs: number) {
    if (!this.session) return
    try {
      const id = this._id++
      const pub = await this.session.declarePublisher(new KeyExpr(keyExpr))
      this.pubs.push({ id, keyExpr, pub, payload, periodMs, sent: 0 })
      const live = this.pubs[this.pubs.length - 1] // reactive proxy
      if (periodMs > 0) {
        live.timer = setInterval(() => { pub.put(live.payload); live.sent++; this.emitFlow({ node: 'pub', id, dir: 'out', topic: keyExpr }) }, periodMs)
        this.note('send', `Declared a publisher on “${keyExpr}”, auto-publishing every ${periodMs} ms.`)
      } else {
        this.note('send', `Declared a manual publisher on “${keyExpr}” — hit “publish now” to put.`)
      }
    } catch (e: any) { this.note('error', `Couldn't declare publisher: ${humanError(e)}`) }
  },

  async publishNow(id: number) {
    const p = this.pubs.find(p => p.id === id)
    if (!p) return
    try { await p.pub.put(p.payload); p.sent++; this.emitFlow({ node: 'pub', id, dir: 'out', topic: p.keyExpr }); this.note('send', `put → “${p.keyExpr}”: ${p.payload}`) }
    catch (e: any) { this.note('error', `put failed: ${humanError(e)}`) }
  },

  // A delightful preset: a declared publisher whose value actually wanders,
  // like a real sensor would.
  async startSensor(keyExpr: string, everyMs = 1000) {
    if (!this.session) return
    try {
      const id = this._id++
      const pub = await this.session.declarePublisher(new KeyExpr(keyExpr))
      this.pubs.push({ id, keyExpr, pub, payload: '{"celsius": …}', periodMs: everyMs, sent: 0 })
      const live = this.pubs[this.pubs.length - 1]
      const base = 21
      live.timer = setInterval(() => {
        const v = (base + Math.sin(now() / 4000) * 4 + (rand() - 0.5)).toFixed(2)
        live.payload = `{"celsius": ${v}}`
        pub.put(live.payload); live.sent++
        this.emitFlow({ node: 'pub', id, dir: 'out', topic: keyExpr })
      }, everyMs)
      this.note('send', `Started a pretend temperature sensor on “${keyExpr}” (every ${everyMs} ms).`)
    } catch (e: any) { this.note('error', `Couldn't start the sensor: ${humanError(e)}`) }
  },

  async stopPublisher(id: number) {
    const i = this.pubs.findIndex(p => p.id === id)
    if (i < 0) return
    const p = this.pubs[i]
    if (p.timer) clearInterval(p.timer)
    try { await p.pub.undeclare() } catch { /* ignore */ }
    this.pubs.splice(i, 1)
    this.note('system', `Undeclared the publisher on “${p.keyExpr}”.`)
  },

  // --- Subscribe: hear everything on a topic (wildcards welcome) ------------
  async subscribe(keyExpr: string) {
    if (!this.session) return
    try {
      const id = this._id++
      // Push first, then mutate through the REACTIVE proxy the array now holds.
      // Mutating the raw object the closure captured would not trigger Vue
      // reactivity, so the feed would fill but the UI would never update.
      this.subs.push({ id, keyExpr, sub: null as any, feed: [], count: 0 })
      const live = this.subs[this.subs.length - 1]
      const sub = await this.session.declareSubscriber(new KeyExpr(keyExpr), {
        handler: (sample: Sample) => {
          live.feed.unshift({
            seq: this._seq++,
            topic: sample.keyexpr().toString(),
            payload: sample.payload().toString(),
            kind: SampleKind[sample.kind()] ?? 'PUT',
            at: now(), fresh: true,
          })
          live.count++
          if (live.feed.length > 40) live.feed.length = 40
          const justLanded = live.feed[0]
          setTimeout(() => { justLanded.fresh = false }, 900)
          // CONCRETE delivered topic (not the subscriber pattern) so the viz can
          // draw a direct edge from the actual producer of this message.
          this.emitFlow({ node: 'sub', id, dir: 'in', topic: sample.keyexpr().toString() })
        },
      })
      live.sub = sub
      this.note('recv', `Listening on “${keyExpr}”. Anything published here will land below.`)
    } catch (e: any) { this.note('error', `Couldn't subscribe: ${humanError(e)}`) }
  },

  async unsubscribe(id: number) {
    const i = this.subs.findIndex(s => s.id === id)
    if (i < 0) return
    const s = this.subs[i]
    try { await s.sub.undeclare() } catch { /* ignore */ }
    this.subs.splice(i, 1)
    this.note('system', `Stopped listening on “${s.keyExpr}”.`)
  },

  // --- Transform: subscribe to A, reshape each payload, republish to B -------
  // This is the goat thesis made tangible: one producer's format becomes
  // another consumer's format without either side knowing about the other.
  // `code` is a JS arrow expression string, `msg => …`, where msg is the
  // incoming payload string and the return is the outgoing payload. It runs
  // in `new Function` — fine here: a throwaway, isolated, no-real-data sandbox.
  async startTransform(inExpr: string, outTopic: string, code: string, label: string) {
    if (!this.session) return
    if (inExpr === outTopic) {
      this.note('error', `Input and output topic are identical — that would loop forever. Give the output a different topic.`)
      return
    }
    let fn: (msg: string) => unknown
    try {
      // eslint-disable-next-line no-new-func
      fn = new Function('msg', `return ( ${code} )(msg)`) as (msg: string) => unknown
      fn('{"probe":1}') // fail fast on syntax errors before declaring anything
    } catch (e: any) {
      this.note('error', `Transform code didn't parse: ${e.message}`)
      return
    }
    try {
      const id = this._id++
      this.xforms.push({ id, inExpr, outTopic, label, sub: null as any, count: 0, last: null, error: '' })
      const live = this.xforms[this.xforms.length - 1]
      const sub = await this.session.declareSubscriber(new KeyExpr(inExpr), {
        handler: (sample: Sample) => {
          const inPayload = sample.payload().toString()
          this.emitFlow({ node: 'xform', id, dir: 'in', topic: sample.keyexpr().toString() })
          try {
            const out = String(fn(inPayload))
            this.session!.put(outTopic, out)
            live.count++; live.last = { in: inPayload, out }; live.error = ''
            this.emitFlow({ node: 'xform', id, dir: 'out', topic: outTopic })
          } catch (e: any) { live.error = e.message }
        },
      })
      live.sub = sub
      this.note('xform', `Transform live: “${inExpr}” → [${label}] → “${outTopic}”. Subscribe to “${outTopic}” to see the new format.`)
    } catch (e: any) { this.note('error', `Couldn't start transform: ${humanError(e)}`) }
  },

  async stopTransform(id: number) {
    const i = this.xforms.findIndex(x => x.id === id)
    if (i < 0) return
    const x = this.xforms[i]
    try { await x.sub.undeclare() } catch { /* ignore */ }
    this.xforms.splice(i, 1)
    this.note('system', `Stopped the transform “${x.inExpr}” → “${x.outTopic}”.`)
  },

  // --- Query (get): send a one-off query, collect the replies ---------------
  async sendQuery(selector: string, onReply: (topic: string, payload: string, ok: boolean) => void) {
    if (!this.session) return
    this.note('query', `get → query “${selector}” — collecting replies from any matching queryable…`)
    try {
      await this.session.get(selector, {
        handler: (reply: Reply) => {
          const r = reply.result()
          if (r instanceof Sample) { onReply(r.keyexpr().toString(), r.payload().toString(), true); return }
          // remote_api surfaces the query-window close as a "Timeout" reply-error.
          // That's not a failure a learner should see as ⚠ — swallow it.
          const errText = (r as any).payload().toString()
          if (/timeout/i.test(errText)) return
          onReply(selector, errText, false)
        },
      })
    } catch (e: any) { this.note('error', `get failed: ${humanError(e)}`) }
  },

  // --- Queryable: declare a queryable that replies to matching queries ------
  async declareQueryable(keyExpr: string, replyText: string) {
    if (!this.session) return
    try {
      const id = this._id++
      this.queryables.push({ id, keyExpr, reply: replyText, q: null as any, replied: 0 })
      const live = this.queryables[this.queryables.length - 1] // reactive proxy
      const q = await this.session.declareQueryable(new KeyExpr(keyExpr), {
        complete: true,
        handler: (query: Query) => {
          query.reply(new KeyExpr(keyExpr), live.reply)
          live.replied++
          this.emitFlow({ node: 'queryable', id, dir: 'out', topic: keyExpr })
          this.note('reply', `queryable replied to a query on “${keyExpr}” with: ${live.reply}`)
        },
      })
      live.q = q
      this.note('reply', `Declared a queryable on “${keyExpr}” — it will reply to matching queries.`)
    } catch (e: any) { this.note('error', `Couldn't declare queryable: ${humanError(e)}`) }
  },

  async undeclareQueryable(id: number) {
    const i = this.queryables.findIndex(q => q.id === id)
    if (i < 0) return
    const st = this.queryables[i]
    try { await st.q.undeclare() } catch { /* ignore */ }
    this.queryables.splice(i, 1)
    this.note('system', `Undeclared the queryable on “${st.keyExpr}”.`)
  },
})

function rand() { return (now() % 1000) / 1000 }

function humanError(e: any): string {
  const m = (e && (e.message || e.toString())) || 'unknown error'
  if (/closed|connect|websocket|network/i.test(m)) {
    return `${m}. Is the server URL right, and are you on the nova mesh?`
  }
  return m
}

export function defaultUrl(): string {
  // Same-origin by default: connect wss to /ws on whatever host served this
  // page (that's how it's wired on nova-a). Falls back to localhost for dev.
  try {
    const loc = window.location
    if (loc.hostname && loc.hostname !== 'localhost' && loc.hostname !== '127.0.0.1') {
      const proto = loc.protocol === 'https:' ? 'wss' : 'ws'
      return `${proto}://${loc.host}/ws`
    }
  } catch { /* SSR-safe noop */ }
  return 'ws://localhost:10000'
}
