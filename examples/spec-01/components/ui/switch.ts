import '@ui-sdk/web/switch'
import { haptic } from '@/lib/haptics'
import { define, escape, Kit } from '@/lib/kit'

class Switch extends Kit {
  #switch?: HTMLElementTagNameMap['ui-switch']

  get checked() {
    return this.#switch?.checked ?? this.hasAttribute('default-checked')
  }

  set checked(checked: boolean) {
    if (this.#switch) this.#switch.checked = checked
    else this.toggleAttribute('default-checked', checked)
  }

  render() {
    const label = escape(this.getAttribute('label'))
    this.dataset.kit = 'switch-row'
    this.innerHTML = `<span data-kit="label">${label}</span><ui-switch data-kit="switch" aria-label="${label}"${this.hasAttribute('default-checked') ? ' default-checked' : ''}><ui-switch-thumb data-kit="switch-thumb"></ui-switch-thumb></ui-switch>`
    const el = (this.#switch = this.querySelector('ui-switch')!)
    let drag: { x: number; moved: boolean; from: boolean; at: number } | null = null
    let ignoreClick = false
    const set = (next: boolean) => {
      if (next !== el.checked) haptic('medium')
      el.checked = next
      this.emit('checkedchange', next)
    }
    this.addEventListener('click', (event) => {
      if (!el.contains(event.target as Node)) el.click()
    })
    el.addEventListener('checkedchange', (event) => {
      if (ignoreClick) return event.preventDefault()
      haptic('medium')
      this.emit('checkedchange', event.detail.value)
    })
    el.addEventListener('pointerdown', (event) => {
      if (event.button === 0) drag = { x: event.clientX, moved: false, from: el.checked, at: el.checked ? 1 : 0 }
    })
    el.addEventListener('pointermove', (event) => {
      if (!drag) return
      const dx = event.clientX - drag.x
      if (!drag.moved) {
        if (Math.abs(dx) < 3) return
        drag.moved = true
        el.setPointerCapture(event.pointerId)
        el.dataset.dragging = ''
      }
      const thumb = el.firstElementChild as HTMLElement
      const travel = el.clientWidth - thumb.offsetWidth - thumb.offsetLeft * 2
      drag.at = Math.min(1, Math.max(0, (drag.from ? 1 : 0) + dx / travel))
      el.style.setProperty('--switch-drag-x', `${drag.at * travel}px`)
    })
    el.addEventListener('pointerup', () => {
      const current = drag
      drag = null
      if (!current?.moved) return
      delete el.dataset.dragging
      el.style.removeProperty('--switch-drag-x')
      set(current.at > 0.5)
      ignoreClick = true
      setTimeout(() => (ignoreClick = false))
    })
  }
}

define('spec-switch', Switch)
