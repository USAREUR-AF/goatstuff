// SPDX-License-Identifier: Apache-2.0
//
// Adaptive performance monitor. Tracks frame-render times via
// requestAnimationFrame; classifies the current operating tier
// (LIVE / SLOW / DEGRADED / AUSTERE) based on rolling-average
// frame budget overflow. All animation, polling, and DOM-update
// frequency in the rest of the app is gated on the current tier so
// the COP keeps functioning on degraded browsers / networks instead
// of locking up.
//
// Tier definitions:
//   LIVE      — frame avg <33ms (≈30fps+). Default full-fidelity.
//   SLOW      — frame avg 33–100ms. Disable packet-dot animation.
//   DEGRADED  — frame avg 100–300ms. Pause force layout, slow
//               polling, hide labels by default.
//   AUSTERE   — frame avg >300ms. Stripped to bare essentials: manual
//               refresh only, static view. Not a call to action — the
//               COP keeps functioning under the worst of conditions.
//
// Companion to the fleet-rate adaptive throttle in render.ts — that
// gates on data volume; this gates on the device's ability to keep
// up. Both can independently demote functionality.

export type PerfTier = "live" | "slow" | "degraded" | "austere";

export interface PerfStats {
  tier: PerfTier;
  avgFrameMs: number;
  worstFrameMs: number;
  framesPerSec: number;
}

type Listener = (s: PerfStats) => void;

export class PerformanceMonitor {
  private samples: number[] = [];
  private worstFrameMs = 0;
  private lastFrameTime = performance.now();
  private listeners: Listener[] = [];
  private tier: PerfTier = "live";
  private running = false;
  private readonly windowSize = 30;

  start(): void {
    if (this.running) return;
    this.running = true;
    this.lastFrameTime = performance.now();
    this.tick();
    // Re-evaluate tier + notify every 2s rather than every frame to
    // avoid noisy oscillation right at tier boundaries.
    window.setInterval(() => this.evaluate(), 2000);
  }

  onChange(listener: Listener): void {
    this.listeners.push(listener);
  }

  current(): PerfStats {
    return {
      tier: this.tier,
      avgFrameMs: this.avg(),
      worstFrameMs: this.worstFrameMs,
      framesPerSec: this.samples.length > 0
        ? Math.round(1000 / Math.max(1, this.avg()))
        : 0,
    };
  }

  private tick = (): void => {
    if (!this.running) return;
    const now = performance.now();
    const delta = now - this.lastFrameTime;
    this.lastFrameTime = now;
    this.samples.push(delta);
    if (this.samples.length > this.windowSize) this.samples.shift();
    if (delta > this.worstFrameMs) this.worstFrameMs = delta;
    requestAnimationFrame(this.tick);
  };

  private avg(): number {
    if (this.samples.length === 0) return 0;
    return this.samples.reduce((a, b) => a + b, 0) / this.samples.length;
  }

  private evaluate(): void {
    const a = this.avg();
    let newTier: PerfTier;
    if (a > 300) newTier = "austere";
    else if (a > 100) newTier = "degraded";
    else if (a > 33) newTier = "slow";
    else newTier = "live";

    // Reset worst-frame tracker between evaluations so a single
    // freeze doesn't haunt the tier forever.
    this.worstFrameMs = 0;
    if (newTier !== this.tier) {
      this.tier = newTier;
      const stats = this.current();
      for (const l of this.listeners) l(stats);
    }
  }
}

/** Resource-policy lookup table — single source of truth for what
 *  each tier permits. Consult per-feature: pollers, animations, and
 *  DOM-render code all read the same table so behavior is consistent. */
export const POLICY: Record<PerfTier, {
  snapshotPollMs: number;
  streamPollMs: number;
  packetDots: boolean;
  forceSim: boolean;
  showLabelsByDefault: boolean;
  badgeColor: string;
  badgeText: string;
}> = {
  live: {
    snapshotPollMs: 1500,
    streamPollMs: 250,
    packetDots: true,
    forceSim: true,
    showLabelsByDefault: false,
    badgeColor: "var(--accent)",
    badgeText: "LIVE",
  },
  slow: {
    snapshotPollMs: 3000,
    streamPollMs: 750,
    packetDots: false,
    forceSim: true,
    showLabelsByDefault: false,
    badgeColor: "var(--schema-ecn)",
    badgeText: "SLOW",
  },
  degraded: {
    snapshotPollMs: 5000,
    streamPollMs: 10000,
    packetDots: false,
    forceSim: false,
    showLabelsByDefault: false,
    badgeColor: "var(--schema-ecn)",
    badgeText: "DEGRADED",
  },
  austere: {
    snapshotPollMs: 30000,
    streamPollMs: 0,
    packetDots: false,
    forceSim: false,
    showLabelsByDefault: false,
    badgeColor: "var(--schema-cot)",
    badgeText: "AUSTERE",
  },
};
