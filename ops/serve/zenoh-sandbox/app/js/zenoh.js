// SPDX-License-Identifier: Apache-2.0
// The heart of the sandbox: one Zenoh session, and the live state of
// everything the visitor has declared on it (publishers, subscribers,
// queryables, transforms) plus a friendly activity log. Built on
// @eclipse-zenoh/zenoh-ts, loaded from the single vendored file in
// ../vendor/zenoh-ts/. Every method maps to one Zenoh concept so the UI can
// stay narrative.
//
// API notes (zenoh-ts 1.9.0):
//  - Session.open(new Config(url)) — url is a FULL ws(s):// URL incl. path;
//    the client returns such URLs verbatim, so a wss://host/ws path survives.
//  - declareSubscriber / get / declareQueryable accept a *callback* handler.
//  - sample.keyexpr().toString(), sample.payload().toString(), sample.kind().
//
// State changes are announced to the panels through onChange(); a panel
// re-renders from `z` when told. Several changes in one turn make one render.
import { Config, Session, KeyExpr, Sample, SampleKind } from '../vendor/zenoh-ts/zenoh-ts.js'

function now() { return Date.now() }

const changeListeners = []
let renderQueued = false

// Register a render callback; it runs once after any batch of state changes.
export function onChange(cb) { changeListeners.push(cb) }

function changed() {
  if (renderQueued) return
  renderQueued = true
  queueMicrotask(() => {
    renderQueued = false
    for (const cb of changeListeners) cb()
  })
}

// status: 'idle' | 'connecting' | 'connected' | 'error'
// log tone: 'info' | 'send' | 'recv' | 'query' | 'reply' | 'xform' | 'error' | 'system'
export const z = {
  url: defaultUrl(),
  status: 'idle',
  error: '',
  session: null,

  subs: [],       // { id, keyExpr, sub, feed: [{ seq, topic, payload, kind, at, fresh }], count }
  pubs: [],       // { id, keyExpr, pub, payload, periodMs, sent, timer? }
  queryables: [], // { id, keyExpr, reply, q, replied }
  xforms: [],     // { id, inExpr, outTopic, label, sub, count, last: { in, out } | null, error }
  log: [],        // { id, at, tone, text }
  logVersion: 0,

  _seq: 0,
  _id: 0,
  _logId: 0,

  // Flow-event bus for the topology panel: one event per observable moment of
  // data moving, { node: 'pub' | 'sub' | 'queryable' | 'xform', id, dir, topic }.
  // dir 'out' = a node sent (a publish or a reply); dir 'in' = a node received.
  // These fire many times a second and do not trigger a panel render.
  _flow: [],
  onFlow(cb) { this._flow.push(cb) },
  emitFlow(e) { for (const cb of this._flow) cb(e) },

  get connected() { return this.status === 'connected' && !!this.session },

  note(tone, text) {
    this.log.unshift({ id: this._logId++, at: now(), tone, text })
    if (this.log.length > 300) this.log.length = 300
    this.logVersion++
    changed()
  },

  async connect() {
    if (this.status === 'connecting' || this.connected) return
    this.status = 'connecting'; this.error = ''
    this.note('system', `Reaching out to the fabric at ${this.url} …`)
    try {
      this.session = await Session.open(new Config(this.url))
      this.status = 'connected'
      this.note('system', `Connected. You're on the fabric. 🎉`)
    } catch (e) {
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
  async putOnce(keyExpr, payload) {
    if (!this.session) return
    try {
      await this.session.put(keyExpr, payload)
      // id -1 = an ad-hoc put with no declared-publisher node; the topology
      // panel draws it from off-stage. dir 'out' carries the concrete topic so
      // the panel fans it out to the matching consumers.
      this.emitFlow({ node: 'pub', id: -1, dir: 'out', topic: keyExpr })
      this.note('send', `put → “${keyExpr}”: ${payload}`)
    } catch (e) { this.note('error', `put failed: ${humanError(e)}`) }
  },

  // Declare a publisher on a topic. periodMs === 0 → manual (use publishNow);
  // periodMs > 0 → auto-publishes the payload every periodMs.
  async declarePublisher(keyExpr, payload, periodMs) {
    if (!this.session) return
    try {
      const id = this._id++
      const pub = await this.session.declarePublisher(new KeyExpr(keyExpr))
      const live = { id, keyExpr, pub, payload, periodMs, sent: 0 }
      this.pubs.push(live)
      if (periodMs > 0) {
        live.timer = setInterval(() => {
          pub.put(live.payload); live.sent++
          this.emitFlow({ node: 'pub', id, dir: 'out', topic: keyExpr })
          changed()
        }, periodMs)
        this.note('send', `Declared a publisher on “${keyExpr}”, auto-publishing every ${periodMs} ms.`)
      } else {
        this.note('send', `Declared a manual publisher on “${keyExpr}” — hit “publish now” to put.`)
      }
    } catch (e) { this.note('error', `Couldn't declare publisher: ${humanError(e)}`) }
  },

  async publishNow(id) {
    const p = this.pubs.find(p => p.id === id)
    if (!p) return
    try {
      await p.pub.put(p.payload); p.sent++
      this.emitFlow({ node: 'pub', id, dir: 'out', topic: p.keyExpr })
      this.note('send', `put → “${p.keyExpr}”: ${p.payload}`)
    } catch (e) { this.note('error', `put failed: ${humanError(e)}`) }
  },

  // A preset: a declared publisher whose value actually wanders, like a real
  // sensor would.
  async startSensor(keyExpr, everyMs = 1000) {
    if (!this.session) return
    try {
      const id = this._id++
      const pub = await this.session.declarePublisher(new KeyExpr(keyExpr))
      const live = { id, keyExpr, pub, payload: '{"celsius": …}', periodMs: everyMs, sent: 0 }
      this.pubs.push(live)
      const base = 21
      live.timer = setInterval(() => {
        const v = (base + Math.sin(now() / 4000) * 4 + (rand() - 0.5)).toFixed(2)
        live.payload = `{"celsius": ${v}}`
        pub.put(live.payload); live.sent++
        this.emitFlow({ node: 'pub', id, dir: 'out', topic: keyExpr })
        changed()
      }, everyMs)
      this.note('send', `Started a pretend temperature sensor on “${keyExpr}” (every ${everyMs} ms).`)
    } catch (e) { this.note('error', `Couldn't start the sensor: ${humanError(e)}`) }
  },

  async stopPublisher(id) {
    const i = this.pubs.findIndex(p => p.id === id)
    if (i < 0) return
    const p = this.pubs[i]
    if (p.timer) clearInterval(p.timer)
    try { await p.pub.undeclare() } catch { /* ignore */ }
    this.pubs.splice(i, 1)
    this.note('system', `Undeclared the publisher on “${p.keyExpr}”.`)
  },

  // --- Subscribe: hear everything on a topic (wildcards welcome) ------------
  async subscribe(keyExpr) {
    if (!this.session) return
    const live = { id: this._id++, keyExpr, sub: null, feed: [], count: 0 }
    try {
      const sub = await this.session.declareSubscriber(new KeyExpr(keyExpr), {
        handler: (sample) => {
          const item = {
            seq: this._seq++,
            topic: sample.keyexpr().toString(),
            payload: sample.payload().toString(),
            kind: SampleKind[sample.kind()] ?? 'PUT',
            at: now(), fresh: true,
          }
          live.feed.unshift(item)
          live.count++
          if (live.feed.length > 40) live.feed.length = 40
          setTimeout(() => { item.fresh = false; changed() }, 900)
          // CONCRETE delivered topic (not the subscriber pattern) so the
          // topology panel can match it against the producer of this message.
          this.emitFlow({ node: 'sub', id: live.id, dir: 'in', topic: item.topic })
          changed()
        },
      })
      live.sub = sub
      this.subs.push(live)
      this.note('recv', `Listening on “${keyExpr}”. Anything published here will land below.`)
    } catch (e) { this.note('error', `Couldn't subscribe: ${humanError(e)}`) }
  },

  async unsubscribe(id) {
    const i = this.subs.findIndex(s => s.id === id)
    if (i < 0) return
    const s = this.subs[i]
    try { await s.sub.undeclare() } catch { /* ignore */ }
    this.subs.splice(i, 1)
    this.note('system', `Stopped listening on “${s.keyExpr}”.`)
  },

  // --- Transform: subscribe to A, reshape each payload, republish to B -------
  // One producer's format becomes another consumer's format without either
  // side knowing about the other. `code` is a JS arrow expression string,
  // `msg => …`, where msg is the incoming payload string and the return value
  // is the outgoing payload.
  async startTransform(inExpr, outTopic, code, label) {
    if (!this.session) return
    if (inExpr === outTopic) {
      this.note('error', `Input and output topic are identical — that would loop forever. Give the output a different topic.`)
      return
    }
    let fn
    try {
      // Deliberate: new Function compiles the function the visitor typed, and it runs in their own browser tab.
      fn = new Function('msg', `return ( ${code} )(msg)`)
      fn('{"probe":1}') // fail fast on syntax errors before declaring anything
    } catch (e) {
      this.note('error', `Transform code didn't parse: ${e.message}`)
      return
    }
    const live = { id: this._id++, inExpr, outTopic, label, sub: null, count: 0, last: null, error: '' }
    try {
      const sub = await this.session.declareSubscriber(new KeyExpr(inExpr), {
        handler: (sample) => {
          const inPayload = sample.payload().toString()
          this.emitFlow({ node: 'xform', id: live.id, dir: 'in', topic: sample.keyexpr().toString() })
          try {
            const out = String(fn(inPayload))
            this.session.put(outTopic, out)
            live.count++; live.last = { in: inPayload, out }; live.error = ''
            this.emitFlow({ node: 'xform', id: live.id, dir: 'out', topic: outTopic })
          } catch (e) { live.error = e.message }
          changed()
        },
      })
      live.sub = sub
      this.xforms.push(live)
      this.note('xform', `Transform live: “${inExpr}” → [${label}] → “${outTopic}”. Subscribe to “${outTopic}” to see the new format.`)
    } catch (e) { this.note('error', `Couldn't start transform: ${humanError(e)}`) }
  },

  async stopTransform(id) {
    const i = this.xforms.findIndex(x => x.id === id)
    if (i < 0) return
    const x = this.xforms[i]
    try { await x.sub.undeclare() } catch { /* ignore */ }
    this.xforms.splice(i, 1)
    this.note('system', `Stopped the transform “${x.inExpr}” → “${x.outTopic}”.`)
  },

  // --- Query (get): send a one-off query, collect the replies ---------------
  // onReply(topic, payload, ok) runs once per reply.
  async sendQuery(selector, onReply) {
    if (!this.session) return
    this.note('query', `get → query “${selector}” — collecting replies from any matching queryable…`)
    try {
      await this.session.get(selector, {
        handler: (reply) => {
          const r = reply.result()
          if (r instanceof Sample) { onReply(r.keyexpr().toString(), r.payload().toString(), true); return }
          // The remote-api router reports the close of the query window as a
          // "Timeout" reply-error. That is the normal end of a get, not a
          // failure to show the visitor.
          const errText = r.payload().toString()
          if (/timeout/i.test(errText)) return
          onReply(selector, errText, false)
        },
      })
    } catch (e) { this.note('error', `get failed: ${humanError(e)}`) }
  },

  // --- Queryable: declare a queryable that replies to matching queries ------
  async declareQueryable(keyExpr, replyText) {
    if (!this.session) return
    const live = { id: this._id++, keyExpr, reply: replyText, q: null, replied: 0 }
    try {
      const q = await this.session.declareQueryable(new KeyExpr(keyExpr), {
        complete: true,
        handler: (query) => {
          query.reply(new KeyExpr(keyExpr), live.reply)
          live.replied++
          this.emitFlow({ node: 'queryable', id: live.id, dir: 'out', topic: keyExpr })
          this.note('reply', `queryable replied to a query on “${keyExpr}” with: ${live.reply}`)
        },
      })
      live.q = q
      this.queryables.push(live)
      this.note('reply', `Declared a queryable on “${keyExpr}” — it will reply to matching queries.`)
    } catch (e) { this.note('error', `Couldn't declare queryable: ${humanError(e)}`) }
  },

  async undeclareQueryable(id) {
    const i = this.queryables.findIndex(q => q.id === id)
    if (i < 0) return
    const st = this.queryables[i]
    try { await st.q.undeclare() } catch { /* ignore */ }
    this.queryables.splice(i, 1)
    this.note('system', `Undeclared the queryable on “${st.keyExpr}”.`)
  },
}

function rand() { return (now() % 1000) / 1000 }

function humanError(e) {
  const m = (e && (e.message || e.toString())) || 'unknown error'
  if (/closed|connect|websocket|network/i.test(m)) {
    return `${m}. Is the server URL right, and are you on the goatnet's mesh?`
  }
  return m
}

export function defaultUrl() {
  // Same-origin by default: connect to /ws on whatever host served this page.
  // On localhost, connect straight to a local router on port 10000.
  const loc = window.location
  if (loc.hostname && loc.hostname !== 'localhost' && loc.hostname !== '127.0.0.1') {
    const proto = loc.protocol === 'https:' ? 'wss' : 'ws'
    return `${proto}://${loc.host}/ws`
  }
  return 'ws://localhost:10000'
}
