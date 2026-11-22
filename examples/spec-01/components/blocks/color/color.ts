import { formatColor, parseColor } from '@ui-sdk/core/color'
import '@/components/ui/button'
import '@/components/ui/device'
import '@/components/ui/display'
import '@/components/ui/flat-knob'
import '@/components/ui/mode-dial'
import { haptic } from '@/lib/haptics'

type Lch = { l: number; c: number; h: number }

const maxChroma = 0.37
const css = ({ l, c, h }: Lch) => `oklch(${l.toFixed(3)} ${c.toFixed(3)} ${h.toFixed(1)})`

function harmony(color: Lch, mode: string): Lch[] {
  const turn = (by: number, l = color.l, c = color.c) => ({ l, c, h: (color.h + by + 360) % 360 })
  if (mode === 'Mono') return [0.3, 0.45, color.l, 0.8, 0.92].map((l) => ({ ...color, l, c: color.c * (l > 0.85 ? 0.5 : 1) }))
  if (mode === 'Ana') return [-40, -20, 0, 20, 40].map((by) => turn(by))
  if (mode === 'Comp') return [turn(0), turn(0, color.l * 0.7), turn(180), turn(180, Math.min(0.92, color.l * 1.25), color.c * 0.6), turn(0, 0.94, color.c * 0.2)]
  return [turn(0), turn(120), turn(240), turn(0, color.l * 0.65), turn(0, 0.94, color.c * 0.2)]
}

export default function mount(root: HTMLElement) {
  const color: Lch = { l: 0.68, c: 0.18, h: 47 }
  let mode = 'Ana'
  let timer = 0

  root.innerHTML = `
    <spec-device model="CX-1" name="Color" plate="none">
      <div data-color-body>
        <div data-color-wing="left">
          <section data-section data-named aria-label="Harmony" data-color-dial>
            <spec-mode-dial label="Harmony" sweep="150" value="${mode}">${['Mono', 'Ana', 'Comp', 'Tri'].map((m) => `<option value="${m}">${m}</option>`).join('')}</spec-mode-dial>
          </section>
          <spec-button data-color-random>Random</spec-button>
        </div>
        <div data-color-center>
          <spec-display aspect="16 / 9">
            <div data-color-screen>
              <svg data-color-ring viewBox="-50 -50 100 100" aria-hidden="true">
                <defs>
                  <filter id="color-glow" x="-60%" y="-60%" width="220%" height="220%">
                    <feDropShadow dx="0" dy="0" stdDeviation="7"></feDropShadow>
                  </filter>
                </defs>
                ${Array.from({ length: 36 }, (_, i) => {
                  const angle = ((i * 10 - 90) * Math.PI) / 180
                  return `<circle cx="${Math.cos(angle) * 42}" cy="${Math.sin(angle) * 42}"></circle>`
                }).join('')}
                <circle r="27" filter="url(#color-glow)"></circle>
              </svg>
              <div data-color-numbers>
                <span data-color-tag>OKLCH</span>
                <dl>
                  <div><dt>L</dt><dd></dd></div>
                  <div><dt>C</dt><dd></dd></div>
                  <div><dt>H</dt><dd></dd></div>
                </dl>
                <span data-color-hex></span>
                <div data-color-palette aria-hidden="true"><i></i><i></i><i></i><i></i><i></i></div>
              </div>
            </div>
          </spec-display>
          <spec-device-plate model="CX-1" name="Color" under></spec-device-plate>
        </div>
        <div data-color-wing="right">
          <div data-color-encoders>
            <spec-flat-knob label="Lightness" color="white"></spec-flat-knob>
            <spec-flat-knob label="Chroma" color="green"></spec-flat-knob>
            <spec-flat-knob label="Hue" color="orange"></spec-flat-knob>
          </div>
          <spec-button tone="primary" data-color-copy>Copy</spec-button>
        </div>
      </div>
    </spec-device>`

  const screen = root.querySelector<HTMLElement>('[data-color-screen]')!
  const glow = root.querySelector('feDropShadow')!
  const circles = root.querySelectorAll('circle')
  const [l, c, h] = root.querySelectorAll('dd')
  const hex = root.querySelector('[data-color-hex]')!
  const swatches = root.querySelectorAll('i')
  const [lightness, chroma, hue] = root.querySelectorAll('spec-flat-knob')
  const copyKey = root.querySelector<HTMLButtonElement>('[data-color-copy]')!

  const showPalette = () => harmony(color, mode).forEach((swatch, i) => (swatches[i].style.background = css(swatch)))
  const show = () => {
    const now = css(color)
    screen.style.setProperty('--now', now)
    glow.setAttribute('flood-color', now)
    for (let i = 0; i < 36; i++) {
      const near = Math.abs(((color.h - i * 10 + 540) % 360) - 180) < 5
      circles[i].setAttribute('r', near ? '4.2' : '2.2')
      circles[i].setAttribute('fill', css({ l: color.l, c: color.c, h: i * 10 }))
      circles[i].setAttribute('stroke', near ? '#f2f2ee' : 'none')
      circles[i].setAttribute('stroke-width', near ? '1' : '0')
    }
    circles[36].setAttribute('fill', now)
    l.textContent = String(Math.round(color.l * 100))
    c.textContent = color.c.toFixed(2)
    h.textContent = String(Math.round(color.h)).padStart(3, '0')
    hex.textContent = formatColor(parseColor(now)!, 'hex')
    showPalette()
  }
  const knobs = () => {
    lightness.value = Math.round(color.l * 100)
    chroma.value = Math.round((color.c / maxChroma) * 100)
    hue.value = Math.round((color.h / 360) * 100)
  }
  show()
  knobs()

  const on = (knob: Element, set: (v: number) => void) =>
    knob.addEventListener('valuechange', (event) => {
      set((event as CustomEvent).detail.value)
      show()
    })
  on(lightness, (v) => (color.l = v / 100))
  on(chroma, (v) => (color.c = (v / 100) * maxChroma))
  on(hue, (v) => (color.h = (v / 100) * 360))

  root.querySelector('spec-mode-dial')!.addEventListener('valuechange', (event) => {
    mode = (event as CustomEvent).detail.value
    showPalette()
  })

  root.querySelector('[data-color-random]')!.addEventListener('click', () => {
    Object.assign(color, { l: 0.55 + Math.random() * 0.3, c: 0.08 + Math.random() * 0.2, h: Math.random() * 360 })
    show()
    knobs()
  })

  copyKey.addEventListener('click', () => {
    void navigator.clipboard?.writeText(css(color))
    haptic('success')
    copyKey.textContent = 'Copied'
    clearTimeout(timer)
    timer = setTimeout(() => (copyKey.textContent = 'Copy'), 1200)
  })

  return () => clearTimeout(timer)
}
