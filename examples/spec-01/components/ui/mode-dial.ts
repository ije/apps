import '@ui-sdk/web/radio'
import { define, escape, Kit } from '@/lib/kit'
import { haptic } from '@/lib/haptics'

class ModeDial extends Kit {
  #group!: HTMLElementTagNameMap['ui-radio-group']
  #value: string | null = null
  #values: string[] = []
  #angles: number[] = []

  render() {
    const options = this.options()
    const sweep = Number(this.getAttribute('sweep') ?? 240)
    const step = options.length > 1 ? sweep / (options.length - 1) : 0
    const value = this.getAttribute('value') ?? this.getAttribute('default-value') ?? options[0].value
    this.#values = options.map((option) => option.value)
    this.#angles = options.map((_, i) => -sweep / 2 + i * step)
    const label = escape(this.getAttribute('label'))
    this.dataset.kit = 'field'
    this.innerHTML = `
      <label data-kit="label">${label}</label>
      <ui-radio-group data-kit="mode-dial" aria-label="${label}" default-value="${escape(value)}" style="--sweep: ${sweep}deg">
        <span data-kit="mode-dial-scale" aria-hidden="true"></span>
        <span data-kit="mode-dial-knob" aria-hidden="true"><span data-kit="mode-dial-pointer"></span></span>
        ${options.map((option, i) => `<ui-radio value="${escape(option.value)}" data-kit="mode-stop" style="--a: ${this.#angles[i]}deg"><span data-kit="mode-stop-label">${escape(option.label)}</span></ui-radio>`).join('')}
      </ui-radio-group>`
    const group = (this.#group = this.querySelector('ui-radio-group')!)
    this.#turn((this.#value = value))
    group.addEventListener('valuechange', (event) => {
      if (event.detail.reason !== 'imperative' && event.detail.value) this.#take(event.detail.value)
    })
    let turning = false
    const turnTo = (event: PointerEvent) => {
      const box = group.getBoundingClientRect()
      const angle = (Math.atan2(event.clientX - box.left - box.width / 2, -(event.clientY - box.top - box.width / 2)) * 180) / Math.PI
      let nearest = 0
      this.#angles.forEach((a, i) => {
        if (Math.abs(a - angle) < Math.abs(this.#angles[nearest] - angle)) nearest = i
      })
      if (group.value !== this.#values[nearest]) group.value = this.#values[nearest]
      this.#take(this.#values[nearest])
    }
    group.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return
      turning = true
      group.setPointerCapture(event.pointerId)
      turnTo(event)
    })
    group.addEventListener('pointermove', (event) => turning && turnTo(event))
    group.addEventListener('pointerup', () => (turning = false))
    group.addEventListener('pointercancel', () => (turning = false))
  }

  get value() {
    return this.rendered ? this.#group.value : (this.getAttribute('value') ?? this.getAttribute('default-value'))
  }

  set value(value: string | null) {
    if (!this.rendered) {
      this.setAttribute('value', value ?? '')
      return
    }
    this.#group.value = this.#value = value
    this.#turn(value)
  }

  #turn(value: string | null) {
    this.#group.style.setProperty('--at', `${this.#angles[Math.max(0, this.#values.indexOf(value!))]}deg`)
  }

  #take(next: string) {
    if (next === this.#value) return
    this.#turn((this.#value = next))
    haptic('selection')
    this.emit('valuechange', next)
  }
}

define('spec-mode-dial', ModeDial)

declare global {
  interface HTMLElementTagNameMap {
    'spec-mode-dial': ModeDial
  }
}
