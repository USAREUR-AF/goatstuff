// SPDX-License-Identifier: Apache-2.0
//
// What a surface does when the sidecar says "not you".
//
// WHY THIS EXISTS. Every gated page here polls, and every poll that outlives
// its session is refused. F-344 made that refusal cheap: oauth2-proxy's
// `API_ROUTES` answers `/api/*` and `/ws` with 401 instead of starting an OIDC
// flow, so an XHR stops minting CSRF cookies. Nothing was taught to READ that
// 401, so a tab whose 8-hour session expired polls at full rate forever and
// shows STALE — the viewer sees a slow feed rather than a signed-out one
// (F-523: 1,048 refused requests in 15 minutes from one laptop, unchanged
// nineteen hours later).
//
// Three rules, one place, so a new poller inherits them instead of
// rediscovering them:
//
//   1. A 401 ends polling. The page reloads ONCE — a top-level navigation is
//      not an API route, so the sidecar redirects it into the sign-in flow and
//      the viewer comes back signed in. If the reload does not fix it, the page
//      says so and stops rather than looping.
//   2. Other failures back off with jitter, capped, so a backend that is down
//      is retried at a decreasing rate rather than hammered.
//   3. A hidden tab does not poll. Nobody is reading it; the cost is the same
//      as a read one.
//
// The scheduling maths is pure and exported for the spec; the DOM effects are
// isolated at the bottom of the file.

/** The status the sidecar returns for a request whose session is gone. */
export const SIGNED_OUT = 401;

/** `403` is NOT signed out: the viewer authenticated and their role does not
 *  cover this view (ADR 1073). Reloading cannot fix it and must not be tried —
 *  `top.ts` already renders that as its own state. */
export function isSignedOut(status: number): boolean {
  return status === SIGNED_OUT;
}

/** Back-off for consecutive FAILURES that are not a sign-out.
 *
 *  Doubles from the poller's own cadence, caps at `capMs`, and carries up to
 *  ±25% jitter so tabs that failed together do not retry together. `attempt` is
 *  the count of consecutive failures; 0 means the last poll succeeded and the
 *  caller keeps its normal cadence.
 *
 *  `rand` is injectable so the spec can pin the jitter. */
export function backoffMs(
  baseMs: number,
  attempt: number,
  capMs = 60_000,
  rand: () => number = Math.random,
): number {
  if (attempt <= 0) return baseMs;
  const grown = Math.min(baseMs * 2 ** Math.min(attempt, 10), capMs);
  const jitter = grown * 0.25 * (rand() * 2 - 1);
  return Math.max(baseMs, Math.round(grown + jitter));
}

/** Run `poll` on a repeating schedule that widens while it keeps failing.
 *
 *  `setInterval` cannot do this: its period is fixed at creation, so a poller
 *  that starts failing keeps firing at full cadence forever, and every open tab
 *  does it at once. That is the amplification `backoffMs` was written for, and
 *  until this helper existed nothing called it — the function shipped in #1135
 *  was reachable only from its own unit test.
 *
 *  `failures` is read AFTER each run so the caller keeps owning the counter it
 *  already maintains for its staleness display; 0 means the last run succeeded
 *  and the normal cadence resumes immediately.
 *
 *  Returns a stop function. The first run is the caller's to make — this only
 *  schedules the ones after it. */
export function pollWithBackoff(
  baseMs: number,
  poll: () => Promise<void> | void,
  failures: () => number,
  capMs = 60_000,
  // Injectable so the spec can drive the schedule without real time. Defaults
  // are the browser's.
  setTimer: (fn: () => void, ms: number) => number = (fn, ms) =>
    window.setTimeout(fn, ms),
  clearTimer: (id: number) => void = (id) => window.clearTimeout(id),
): () => void {
  let timer: number | null = null;
  let stopped = false;
  const arm = (delay: number): void => {
    if (stopped) return;
    timer = setTimer(() => {
      void (async () => {
        try {
          await poll();
        } finally {
          arm(backoffMs(baseMs, failures(), capMs));
        }
      })();
    }, delay);
  };
  arm(baseMs);
  return () => {
    stopped = true;
    if (timer !== null) clearTimer(timer);
    timer = null;
  };
}

/** The count a poller hands `pollWithBackoff` as `failures`.
 *
 *  A 403 is an answer, not a failed request, so `top.ts` and `who.ts` keep it
 *  out of `consecutiveFailures`: that counter drives the staleness display, and
 *  a refused viewer must read "restricted", not "unreachable". But it is also
 *  the one status that cannot succeed on the next tick — a role does not change
 *  between two polls — so left out of the schedule it is re-requested at full
 *  cadence for the life of the tab, which is the amplification this file exists
 *  to stop. The schedule therefore reads whichever streak is longer. A granted
 *  role is picked up within one capped gap, and the first 200 resets both. */
export function pacingFailures(consecutiveFailures: number, restrictedStreak: number): number {
  return Math.max(consecutiveFailures, restrictedStreak);
}

/** Whether a poll should run at all. A hidden tab is skipped — the caller keeps
 *  its timer alive and checks again next tick, so the page resumes on its own
 *  when the viewer comes back, without a visibilitychange listener per poller. */
export function shouldPoll(hidden: boolean, signedOut: boolean): boolean {
  return !hidden && !signedOut;
}

/** One reload, not a loop.
 *
 *  A reload is only worth attempting if we have not just done one: a session
 *  that is still refused immediately after a sign-in round-trip is not going to
 *  be fixed by another. `lastAttemptAt` is 0 when we have never tried. */
export function shouldReload(nowMs: number, lastAttemptAt: number, windowMs = 30_000): boolean {
  return lastAttemptAt === 0 || nowMs - lastAttemptAt > windowMs;
}

// ── the live state, shared by every poller on the page ─────────────────────

const RELOAD_MARK = "panoscope.signin.attempt";
let signedOutNow = false;

/** True once any poller has seen a 401. Pollers check this before fetching, so
 *  one refused request stops the whole page rather than each loop discovering
 *  the same thing. */
export function isPageSignedOut(): boolean {
  return signedOutNow;
}

function readMark(): number {
  try {
    return Number(sessionStorage.getItem(RELOAD_MARK) || 0);
  } catch {
    // Private windows and blocked site data throw on access. Without the mark
    // we cannot tell a first attempt from a second, and the safe side is to
    // show the banner rather than risk a reload loop.
    return Date.now();
  }
}

function writeMark(nowMs: number): void {
  try {
    sessionStorage.setItem(RELOAD_MARK, String(nowMs));
  } catch {
    /* nothing to do: the banner path still works */
  }
}

/** Call this with any response a poller gets. Returns true when the response
 *  was a sign-out, in which case the caller must stop its loop and return —
 *  the page is already navigating or showing the banner. */
export function handleResponse(resp: { status: number }): boolean {
  if (!isSignedOut(resp.status)) return false;
  declareSignedOut();
  return true;
}

/** Enter the signed-out state: stop the page polling, then either reload into
 *  the sign-in flow or say plainly that the session ended. */
export function declareSignedOut(now: () => number = Date.now): void {
  if (signedOutNow) return;
  signedOutNow = true;
  const nowMs = now();
  if (shouldReload(nowMs, readMark())) {
    writeMark(nowMs);
    // Same URL, as a navigation rather than an XHR: `API_ROUTES` does not match
    // it, so the sidecar sends the viewer through OIDC and back to this page.
    location.reload();
    return;
  }
  showSignedOutBanner();
}

/** The honest end state: a reload already failed to restore the session, so
 *  nothing this page can do will. Says which link signs back in and leaves the
 *  stale view on screen rather than blanking it. */
export function showSignedOutBanner(): void {
  if (document.getElementById("signed-out-banner")) return;
  const el = document.createElement("div");
  el.id = "signed-out-banner";
  el.setAttribute("role", "status");
  el.style.cssText =
    "position:fixed;inset-inline:0;top:0;z-index:9999;padding:10px 16px;" +
    "text-align:center;font:14px/1.4 system-ui,sans-serif;" +
    "background:#4a1d1d;color:#ffe8e4;border-bottom:1px solid #8a3a30";
  const link = document.createElement("a");
  link.href = location.pathname + location.search;
  link.textContent = "Sign in again";
  link.style.cssText = "color:#ffd9a8;font-weight:600";
  el.append("Your session ended, so this page stopped updating. ", link);
  el.append(" — what you see below is from before it ended.");
  document.body.prepend(el);
}
