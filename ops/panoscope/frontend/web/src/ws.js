// SPDX-License-Identifier: Apache-2.0
//
// WebSocket client for /ws. Connects, parses OutgoingMessage envelopes,
// dispatches to the caller's handlers. Auto-reconnects on close with
// linear backoff capped at 5s.


/**
 * @typedef {"connecting" | "live" | "lagged" | "closed"} ConnectionState
 */

/**
 * @typedef {Object} WsHandlers
 * @property {(s: import("./types.js").Snapshot) => void} onSnapshot
 * @property {(events: import("./types.js").WsEvent[]) => void} onDelta
 * @property {(missed: number) => void} onLagged
 * @property {(s: ConnectionState) => void} onStateChange
 * @property {() => void} [onReady] Fires after each successful (re)connect — gives the caller a
 *   chance to re-emit any per-connection state the backend doesn't
 *   persist. Used by LIVE-follow to re-emit the current watch on
 *   reconnect (per docs/design/panoscope.md lifecycle invariant).
 * @property {(reason: string, error: unknown) => void} [onError] Fires when an inbound message fails to JSON-parse OR a downstream
 *   handler (onSnapshot / onDelta / onLagged) throws. Lets the UI
 *   surface the failure instead of leaving a half-rendered canvas
 *   with no signal — a single ReferenceError inside renderer.setSnapshot
 *   would otherwise blank the viz while the HUD kept ticking from a
 *   parallel WS-events path, indistinguishable from "fabric is quiet."
 *
 *   `reason` is a short human-readable tag (e.g. "JSON parse failed",
 *   "Handler error on snapshot"); `error` is the original exception.
 */

import { isPageSignedOut } from "./session.js";

export class WsClient {
  url;
  socket = null;
  handlers;
  reconnectAttempts = 0;
  closed = false;

  constructor(url, handlers) {
    this.url = url;
    this.handlers = handlers;
  }

  connect() {
    this.handlers.onStateChange("connecting");
    const socket = new WebSocket(this.url);
    this.socket = socket;

    socket.addEventListener("open", () => {
      this.reconnectAttempts = 0;
      this.handlers.onStateChange("live");
      this.handlers.onReady?.();
    });

    socket.addEventListener("message", (ev) => {
      let msg;
      try {
        msg = JSON.parse(String(ev.data));
      } catch (e) {
        console.error("ws: JSON parse error", e, ev.data);
        this.handlers.onError?.("JSON parse failed", e);
        return;
      }
      // Handler errors get their own catch so a TDZ / type confusion
      // inside a render handler doesn't get conflated with "the wire
      // had garbage." The viz needs both signals routed separately.
      try {
        this.dispatch(msg);
      } catch (e) {
        console.error("ws: handler error", e, "msg.type=", msg.type);
        this.handlers.onError?.(`Handler error on ${msg.type}`, e);
      }
    });

    socket.addEventListener("close", () => {
      if (this.closed) return;
      this.handlers.onStateChange("closed");
      this.scheduleReconnect();
    });

    socket.addEventListener("error", (e) => {
      console.warn("ws: error", e);
    });
  }

  close() {
    this.closed = true;
    this.socket?.close();
  }

  /** Send an InboundMessage to the backend. No-op when the socket
   *  isn't open — callers re-emit on the `onReady` hook after
   *  reconnect, so a drop on closed isn't a lost-message problem. */
  send(msg) {
    if (this.socket && this.socket.readyState === WebSocket.OPEN) {
      this.socket.send(JSON.stringify(msg));
    }
  }

  dispatch(msg) {
    switch (msg.type) {
      case "snapshot":
        this.handlers.onSnapshot(msg);
        break;
      case "delta":
        this.handlers.onDelta(msg.events);
        break;
      case "lagged":
        this.handlers.onLagged(msg.missed);
        this.handlers.onStateChange("lagged");
        break;
    }
  }

  scheduleReconnect() {
    // A socket refused because the session ended closes immediately, so this
    // path is the websocket half of F-523: without the check it reconnects
    // every 5 s for as long as the tab stays open. The page is already
    // navigating to sign-in or showing the banner; stop reconnecting.
    if (isPageSignedOut()) {
      this.closed = true;
      return;
    }
    this.reconnectAttempts += 1;
    const delayMs = Math.min(this.reconnectAttempts * 500, 5000);
    setTimeout(() => {
      if (!this.closed && !isPageSignedOut()) this.connect();
    }, delayMs);
  }
}
