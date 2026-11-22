import '@ui-sdk/web/number-input'
import { define, escape, Kit } from '@/lib/kit'
import { haptic } from '@/lib/haptics'

const ticks = 27
const lit = (value: number) => Math.floor((value * ticks) / 100)
const ring = `<svg data-kit="knob-ticks" viewBox="-50 -50 100 100" aria-hidden="true">${Array.from({ length: ticks + 1 }, (_, i) => {
  const angle = ((-135 + (i / ticks) * 270 - 90) * Math.PI) / 180
  const [x, y] = [Math.cos(angle), Math.sin(angle)]
  return `<line x1="${x * 40}" y1="${y * 40}" x2="${x * 50}" y2="${y * 50}"${i % 9 === 0 ? ' data-major="true"' : ''}></line>`
}).join('')}</svg>`

class FlatKnob extends Kit {
  #input!: HTMLElementTagNameMap['ui-number-input']
  #knob!: HTMLElement
  #lines!: NodeListOf<SVGLineElement>
  #shown = -1

  render() {
    const label = escape(this.getAttribute('label'))
    const value = this.getAttribute('value') ?? this.getAttribute('default-value') ?? '0'
    this.innerHTML = `
      <ui-number-input min="0" max="100" default-value="${escape(value)}">
        <div data-kit="knob" data-color="${escape(this.getAttribute('color'))}">
          <ui-number-input-scrub-area data-kit="knob-dial" orientation="vertical" pixels-per-step="2">${ring}<span data-kit="knob-cap"></span></ui-number-input-scrub-area>
          <ui-number-input-control><input data-kit="knob-readout" aria-label="${label}"></ui-number-input-control>
          <span data-kit="label">${label}</span>
        </div>
      </ui-number-input>`
    this.#input = this.querySelector('ui-number-input')!
    this.#knob = this.querySelector('[data-kit="knob"]')!
    this.#lines = this.querySelectorAll('line')
    this.#paint(Number(value))
    this.#input.addEventListener('valuechange', (event) => {
      const next = event.detail.value
      if (next === null || event.detail.reason === 'imperative') return
      if (lit(this.#shown) !== lit(next)) haptic('selection')
      this.#paint(next)
      this.emit('valuechange', next)
    })
  }

  get value() {
    return this.rendered ? (this.#input.value ?? 0) : Number(this.getAttribute('value') ?? this.getAttribute('default-value') ?? 0)
  }

  set value(value: number) {
    if (!this.rendered) {
      this.setAttribute('value', String(value))
      return
    }
    if (value === this.#input.value) return
    this.#input.value = value
    this.#paint(value)
  }

  #paint(value: number) {
    if (value === this.#shown) return
    const [from, to] = [lit(this.#shown), lit(value)]
    this.#shown = value
    const share = value / 100
    this.#knob.style.setProperty('--turn', `${share * 270 - 135}deg`)
    this.#knob.style.setProperty('--lit', String(share))
    for (let i = Math.min(from, to) + 1; i <= Math.max(from, to); i++) this.#lines[i]?.toggleAttribute('data-lit', i <= to)
  }
}

define('spec-flat-knob', FlatKnob)

declare global {
  interface HTMLElementTagNameMap {
    'spec-flat-knob': FlatKnob
  }
}
