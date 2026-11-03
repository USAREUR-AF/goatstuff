// Shared playback controls for the live, cycling canvas visualizations.
// Gives each viz: play/pause, a speed selector, a draggable SCRUB bar (seek
// anywhere in one scenario's timeline — some beats play fast, some slow, so the
// viewer can slow down / speed up / pause / queue exactly the part they want),
// and a scenario toggle group (default = auto-rotate through all scenarios, or
// pin one). The viz supplies a render(now) callback; this owns the clock + rAF.
import { ref, onMounted, onBeforeUnmount } from 'vue'

export const VIZ_SPEEDS = [0.5, 1, 2, 4]

export function useVizControls(cycleMs: number, sceneCount: number, render: (now: number) => void) {
  const playing = ref(true)
  const speedIdx = ref(1)        // index into VIZ_SPEEDS (1 → 1×)
  const scene = ref(-1)          // -1 = auto-rotate; >=0 = pinned scenario
  const progress = ref(0)        // 0..1 within the current scenario (drives the scrub bar)
  let tNow = 0, last = 0, raf = 0, dragging = false

  function frame() {
    raf = requestAnimationFrame(frame)
    const real = performance.now()
    const dt = last ? Math.min(64, real - last) : 0; last = real   // clamp dt so a backgrounded tab doesn't jump
    if (playing.value && !dragging) tNow += dt * VIZ_SPEEDS[speedIdx.value]
    progress.value = (tNow % cycleMs) / cycleMs
    render(tNow)
  }

  const toggle = () => { playing.value = !playing.value }
  const setSpeed = (i: number) => { speedIdx.value = i }
  const pickScene = (i: number) => { scene.value = i; tNow = Math.floor(tNow / cycleMs) * cycleMs }  // restart the chosen scene
  const scrubTo = (frac: number) => {
    const f = Math.max(0, Math.min(1, frac))
    tNow = Math.floor(tNow / cycleMs) * cycleMs + f * cycleMs
    progress.value = f
  }
  const onScrubStart = () => { dragging = true }
  const onScrubEnd = () => { dragging = false; last = performance.now() }
  const sceneIndex = () => (scene.value >= 0 ? scene.value : Math.floor(tNow / cycleMs) % sceneCount)

  onMounted(() => { last = performance.now(); raf = requestAnimationFrame(frame) })
  onBeforeUnmount(() => cancelAnimationFrame(raf))

  return { playing, speedIdx, scene, progress, toggle, setSpeed, pickScene, scrubTo, onScrubStart, onScrubEnd, sceneIndex }
}
