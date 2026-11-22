import '@ui-sdk/web/radio'
import { define, escape, Kit } from '@/lib/kit'
import { haptic } from '@/lib/haptics'

class SlideSwitch extends Kit {
  #group!: HTMLElementTagNameMap['ui-radio-group']
  #value: string | null = null
  #values: string[] = []

  render() {
    const values = (this.#values = this.options().map((option) => option.value))
    const orientation = this.getAttribute('orientation') ?? 'horizontal'
    const side = this.getAttribute('side') ?? 'end'
    const value = this.getAttribute('value') ?? this.getAttribute('default-value') ?? values[0]
    const label = escape(this.getAttribute('label'))
    this.dataset.kit = 'field'
    this.innerHTML = `
      <label data-kit="label">${label}</label>
      <ui-radio-group data-kit="slide-switch" aria-label="${label}" data-orientation="${orientation}"${orientation === 'vertical' || side === 'start' ? ` data-side="${side}"` : ''} default-value="${escape(value)}" style="--n: ${values.length}">
        <span data-kit="slide-switch-lever" aria-hidden="true"></span>
        ${values.map((option) => `<ui-radio value="${escape(option)}" data-kit="slide-switch-stop">${escape(option)}</ui-radio>`).join('')}
      </ui-radio-group>`
    const group = (this.#group = this.querySelector('ui-radio-group')!)
    this.#slide((this.#value = value))
    group.addEventListener('valuechange', (event) => {
      if (event.detail.reason !== 'imperative' && event.detail.value) this.#take(event.detail.value)
    })
    let dragging = false
    const at = (event: PointerEvent) => {
      const box = group.getBoundingClientRect()
      const share = orientation === 'vertical' ? (event.clientY - box.top) / box.height : (event.clientX - box.left) / box.width
      return Math.min(values.length - 1, Math.max(0, share * values.length - 0.5))
    }
    const choose = (next: string) => {
      if (group.value !== next) group.value = next
      this.#take(next)
    }
    const stop = () => {
      dragging = false
      group.removeAttribute('data-dragging')
      this.#slide(this.#value)
    }
    group.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return
      group.setPointerCapture(event.pointerId)
      dragging = true
      group.setAttribute('data-dragging', '')
      group.style.setProperty('--i', String(at(event)))
    })
    group.addEventListener('pointermove', (event) => {
      if (!dragging) return
      const pos = at(event)
      choose(values[Math.round(pos)])
      group.style.setProperty('--i', String(pos))
    })
    group.addEventListener('pointerup', (event) => {
      if (!dragging) return
      choose(values[Math.round(at(event))])
      stop()
    })
    group.addEventListener('pointercancel', stop)
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
    if (!this.#group.hasAttribute('data-dragging')) this.#slide(value)
  }

  #slide(value: string | null) {
    this.#group.style.setProperty('--i', String(Math.max(0, this.#values.indexOf(value!))))
  }

  #take(next: string) {
    if (next === this.#value) return
    this.#value = next
    if (!this.#group.hasAttribute('data-dragging')) this.#slide(next)
    haptic('selection')
    this.emit('valuechange', next)
  }
}

define('spec-slide-switch', SlideSwitch)

declare global {
  interface HTMLElementTagNameMap {
    'spec-slide-switch': SlideSwitch
  }
}
