// SPDX-License-Identifier: Apache-2.0
// Shared playback controls for the live, cycling canvas visualizations.
// Gives each viz: play/pause, a speed selector, a draggable SCRUB bar (seek
// anywhere in one scenario's timeline — some beats play fast, some slow, so the
// viewer can slow down / speed up / pause / queue exactly the part they want),
// and a scenario toggle group (default = auto-rotate through all scenarios, or
// pin one). The viz supplies a render(now) callback; this owns the clock + rAF.

export const VIZ_SPEEDS = [0.5, 1, 2, 4]

export function createVizControls(cycleMs, sceneCount, render) {
  const st = {
    playing: true,
    speedIdx: 1,   // index into VIZ_SPEEDS (1 → 1×)
    scene: -1,     // -1 = auto-rotate; >=0 = pinned scenario
    progress: 0,   // 0..1 within the current scenario (drives the scrub bar)
  }
  let tNow = 0, last = 0, raf = 0, dragging = false

  function frame() {
    raf = requestAnimationFrame(frame)
    const real = performance.now()
    const dt = last ? Math.min(64, real - last) : 0; last = real   // clamp dt so a backgrounded tab doesn't jump
    if (st.playing && !dragging) tNow += dt * VIZ_SPEEDS[st.speedIdx]
    st.progress = (tNow % cycleMs) / cycleMs
    render(tNow)
  }

  st.toggle = () => { st.playing = !st.playing }
  st.setSpeed = (i) => { st.speedIdx = i }
  st.pickScene = (i) => { st.scene = i; tNow = Math.floor(tNow / cycleMs) * cycleMs }  // restart the chosen scene
  st.scrubTo = (frac) => {
    const f = Math.max(0, Math.min(1, frac))
    tNow = Math.floor(tNow / cycleMs) * cycleMs + f * cycleMs
    st.progress = f
  }
  st.onScrubStart = () => { dragging = true }
  st.onScrubEnd = () => { dragging = false; last = performance.now() }
  st.sceneIndex = () => (st.scene >= 0 ? st.scene : Math.floor(tNow / cycleMs) % sceneCount)
  st.start = () => { last = performance.now(); raf = requestAnimationFrame(frame) }
  st.stop = () => cancelAnimationFrame(raf)
  return st
}
