import '@/components/ui/button'
import '@/components/ui/checkbox'
import '@/components/ui/device'
import '@/components/ui/icon'
import '@/components/ui/lcd'
import '@/components/ui/slide-switch'
import { haptic } from '@/lib/haptics'

const full = 60 * 60

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-device model="TM-1" name="Timer" on="false">
      <div data-timer>
        <div data-timer-dial role="slider" tabindex="0" aria-label="Time" aria-valuemin="1" aria-valuemax="60">
          <span data-timer-wedge aria-hidden="true"></span>
          <svg data-timer-scale viewBox="-100 -100 200 200" aria-hidden="true">${Array.from({ length: 60 }, (_, i) => {
            const a = (i * 6 * Math.PI) / 180
            const inner = i % 5 === 0 ? 85 : 89
            return `<line x1="${Math.sin(a) * inner}" y1="${-Math.cos(a) * inner}" x2="${Math.sin(a) * 95}" y2="${-Math.cos(a) * 95}"${i % 5 === 0 ? ' data-major="true"' : ''}></line>`
          }).join('')}</svg>
          ${[0, 10, 20, 30, 40, 50].map((m) => `<span data-timer-number style="--m: ${m}" aria-hidden="true">${m}</span>`).join('')}
          <span data-timer-hub aria-hidden="true"></span>
        </div>
        <div data-timer-middle>
          <div data-timer-lcd-row>
            <spec-lcd data-timer-lcd size="lg" tag="Set" aria-live="off"><spec-lcd-digits value="15:00"></spec-lcd-digits></spec-lcd>
            <div data-timer-start><spec-button tone="primary" round aria-label="Start"><spec-icon name="play"></spec-icon></spec-button></div>
          </div>
          <div data-timer-row>
            <spec-slide-switch label="Preset" default-value="15"><option>1</option><option>5</option><option>15</option><option>25</option><option>45</option></spec-slide-switch>
            <div data-timer-small>
              <spec-button round aria-label="Reset"><spec-icon name="restart"></spec-icon></spec-button>
              <spec-checkbox label="Chime" icon="bell" default-checked></spec-checkbox>
            </div>
          </div>
        </div>
      </div>
    </spec-device>`
  const device = root.firstElementChild!
  const dial = root.querySelector<HTMLElement>('[data-timer-dial]')!
  const lcd = root.querySelector('spec-lcd')!
  const digits = lcd.querySelector('spec-lcd-digits')!
  const start = root.querySelector<HTMLButtonElement>('[data-timer-start] button')!
  const preset = root.querySelector('spec-slide-switch')!

  let set = 15 * 60
  let left = set
  let running = false
  let chime = true
  let audio: AudioContext | null = null
  let id = 0
  let shown = ''

  const show = () => {
    dial.style.setProperty('--share', String(left / full))
    const minutes = Math.floor(left / 60)
    const seconds = Math.floor(left % 60)
    const text = `${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`
    if (text === shown) return
    digits.setAttribute('value', (shown = text))
    dial.ariaValueNow = String(Math.ceil(left / 60))
    dial.ariaValueText = `${minutes} minutes ${seconds} seconds`
  }

  const state = () => {
    const done = left <= 0
    dial.toggleAttribute('data-done', done)
    digits.toggleAttribute('data-done', done)
    lcd.setAttribute('tag', done ? 'Done' : running ? 'Running' : 'Set')
    device.setAttribute('on', String(running || done))
    start.ariaLabel = running ? 'Pause' : 'Start'
    start.firstElementChild!.setAttribute('name', running ? 'pause' : 'play')
    if (!running) {
      cancelAnimationFrame(id)
      id = 0
    }
    if (!running || id) return
    let last = performance.now()
    id = requestAnimationFrame(function tick(now) {
      left = Math.max(0, left - Math.max(0, now - last) / 1000)
      last = now
      show()
      if (left > 0) {
        id = requestAnimationFrame(tick)
        return
      }
      running = false
      state()
      haptic('success')
      if (!chime || !audio || !AudioContext.prototype.createOscillator) return
      const ctx = audio
      for (let i = 0; i < 3; i++) {
        const osc = ctx.createOscillator()
        const gain = ctx.createGain()
        osc.frequency.value = 1320
        gain.gain.setValueAtTime(0, ctx.currentTime + i * 0.3)
        gain.gain.linearRampToValueAtTime(0.2, ctx.currentTime + i * 0.3 + 0.01)
        gain.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + i * 0.3 + 0.22)
        osc.connect(gain).connect(ctx.destination)
        osc.start(ctx.currentTime + i * 0.3)
        osc.stop(ctx.currentTime + i * 0.3 + 0.25)
      }
    })
  }

  const setTo = (seconds: number, stop: boolean) => {
    if (seconds !== set) haptic('selection')
    set = left = seconds
    if (stop) running = false
    show()
    state()
  }

  const setFrom = (event: PointerEvent) => {
    const box = dial.getBoundingClientRect()
    const angle = Math.atan2(event.clientX - box.left - box.width / 2, -(event.clientY - box.top - box.height / 2))
    setTo(Math.max(60, Math.round((((angle * 180) / Math.PI + 360) % 360) / 6) * 60), true)
  }

  dial.addEventListener('pointerdown', (event) => {
    dial.setPointerCapture(event.pointerId)
    setFrom(event)
  })
  dial.addEventListener('pointermove', (event) => event.buttons && setFrom(event))
  dial.addEventListener('keydown', (event) => {
    const by = ({ ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1 } as Record<string, number>)[event.key]
    if (!by) return
    event.preventDefault()
    setTo(Math.min(full, Math.max(60, (Math.round(left / 60) + by) * 60)), false)
  })
  start.addEventListener('click', () => {
    audio ??= new AudioContext()
    void audio.resume()
    if (left <= 0) left = set
    running = !running
    show()
    state()
  })
  preset.addEventListener('valuechange', (event) => {
    set = left = Number((event as CustomEvent).detail.value) * 60
    running = false
    show()
    state()
  })
  root.querySelector('[aria-label="Reset"]')!.addEventListener('click', () => {
    running = false
    left = set
    show()
    state()
  })
  root.querySelector('spec-checkbox')!.addEventListener('checkedchange', (event) => (chime = (event as CustomEvent).detail.value))
  show()

  return () => {
    cancelAnimationFrame(id)
    void audio?.close()
  }
}
