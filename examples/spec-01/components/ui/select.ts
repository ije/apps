import '@ui-sdk/web/listbox'
import { haptic } from '@/lib/haptics'
import { define, escape, Kit } from '@/lib/kit'

const row = 32
const radius = row / ((22 * Math.PI) / 180)
const pitch = 4
const ridges = Math.ceil((Math.PI * radius) / pitch) + 2

let count = 0

class Select extends Kit {
  #options: { value: string; label: string }[] = []
  #value = ''
  #list!: HTMLElementTagNameMap['ui-listbox']
  #ridges: HTMLElement[] = []
  #faces: HTMLElement[] = []
  #drag: { y: number; top: number; moved: boolean } | null = null
  #dragged = false
  #target: number | null = null
  #settle = 0
  #painting = 0

  render() {
    this.#options = this.options()
    this.#value = this.getAttribute('default-value') ?? this.#options[0]?.value ?? ''
    const rail = (side: string) =>
      `<div data-kit="wheel-rail" data-side="${side}" aria-hidden="true">${Array.from({ length: ridges }, (_, i) => `<i data-ridge="${i}"></i>`).join('')}</div>`
    const id = `spec-select-${++count}`
    this.dataset.kit = 'field'
    this.innerHTML = `
      <label data-kit="label" id="${id}">${escape(this.getAttribute('label'))}</label>
      <div data-kit="wheel-frame" style="--wheel-row: ${row}px">
        ${rail('left')}${rail('right')}
        <ui-listbox data-kit="wheel" aria-labelledby="${id}" default-value="${escape(this.#value)}">
          ${this.#options.map(({ value, label }) => `<ui-listbox-item value="${escape(value)}" data-kit="wheel-item"><span data-kit="wheel-face">${escape(label)}</span></ui-listbox-item>`).join('')}
        </ui-listbox>
      </div>`
    const list = (this.#list = this.querySelector('ui-listbox')!)
    this.#ridges = [...this.querySelectorAll<HTMLElement>('[data-ridge]')]
    this.#faces = [...this.querySelectorAll<HTMLElement>('[data-kit="wheel-face"]')]

    list.addEventListener('valuechange', (event) => {
      const { value, reason } = (event as CustomEvent).detail
      if (reason === 'imperative') return
      if (!value) return event.preventDefault()
      this.#value = value
      this.emit('valuechange', value)
      this.#roll(true)
    })
    list.addEventListener('scroll', () => {
      this.#painting ||= requestAnimationFrame(() => {
        this.#painting = 0
        this.#paint()
      })
      if (this.#target !== null) {
        if (Math.abs(list.scrollTop - this.#target) < 1) this.#target = null
      } else this.#chooseCentered()
      clearTimeout(this.#settle)
      this.#settle = window.setTimeout(() => {
        this.#target = null
        if (this.#drag) return
        this.#chooseCentered()
        if (!list.contains(document.activeElement)) return
        const option = list.querySelectorAll<HTMLElement>('[role="option"]')[Math.round(list.scrollTop / row)]
        if (option && option !== document.activeElement) option.focus({ preventScroll: true })
      }, 120)
    })
    list.addEventListener('pointerdown', (event) => {
      this.#dragged = false
      if (event.pointerType !== 'mouse' || event.button !== 0) return
      this.#drag = { y: event.clientY, top: list.scrollTop, moved: false }
    })
    list.addEventListener('pointermove', (event) => {
      const drag = this.#drag
      if (!drag) return
      const dy = event.clientY - drag.y
      if (!drag.moved) {
        if (Math.abs(dy) < 3) return
        drag.moved = true
        list.setPointerCapture(event.pointerId)
        list.dataset.dragging = ''
      }
      list.scrollTop = drag.top - dy
    })
    list.addEventListener('pointerup', () => {
      const drag = this.#drag
      this.#drag = null
      if (!drag?.moved) return
      delete list.dataset.dragging
      this.#change(this.#options[this.#nearest()].value)
      this.#roll(true)
      this.#dragged = true
    })
    list.addEventListener('pointercancel', () => (this.#drag = null))
    list.addEventListener('mousedown', (event) => {
      const option = (event.target as HTMLElement).closest<HTMLElement>('[role="option"]')
      if (!option) return
      event.preventDefault()
      option.focus({ preventScroll: true })
    })
    list.addEventListener(
      'click',
      (event) => {
        if (!this.#dragged) return
        this.#dragged = false
        event.preventDefault()
        event.stopPropagation()
      },
      true,
    )
    this.#roll(false)
  }

  disconnectedCallback() {
    clearTimeout(this.#settle)
    cancelAnimationFrame(this.#painting)
    this.#painting = 0
  }

  get value() {
    return this.#value
  }

  set value(value: string) {
    if (!this.#list) this.setAttribute('default-value', value)
    else if (value !== this.#value) {
      this.#value = value
      this.#list.value = value
      this.#roll(true)
    }
  }

  #nearest() {
    return Math.max(0, Math.min(this.#options.length - 1, Math.round(this.#list.scrollTop / row)))
  }

  #change(value: string) {
    if (value === this.#value) return false
    this.#value = value
    this.#list.value = value
    this.emit('valuechange', value)
    return true
  }

  #chooseCentered() {
    if (this.#change(this.#options[this.#nearest()].value)) haptic('selection')
  }

  #roll(smooth: boolean) {
    const list = this.#list
    const top = Math.max(0, this.#options.findIndex((option) => option.value === this.#value)) * row
    if (Math.abs(list.scrollTop - top) >= 1 && !this.#drag) {
      this.#target = top
      list.scrollTo({ top, behavior: smooth ? 'smooth' : 'instant' })
    }
    this.#paint()
  }

  #paint() {
    const top = this.#list.scrollTop
    const at = top / row
    const phase = ((top % pitch) + pitch) % pitch
    const half = ridges / 2
    for (const ridge of this.#ridges) {
      const angle = ((Number(ridge.dataset.ridge) - half) * pitch - phase) / radius
      const visible = Math.abs(angle) < Math.PI / 2
      ridge.style.transform = visible
        ? `translateY(${(radius * Math.sin(angle)).toFixed(2)}px) scaleY(${Math.cos(angle).toFixed(3)})`
        : 'scaleY(0)'
      ridge.style.opacity = visible ? (0.2 + 0.8 * Math.cos(angle)).toFixed(2) : '0'
    }
    this.#faces.forEach((face, i) => {
      const d = i - at
      const angle = Math.max(-90, Math.min(90, d * 22))
      const rad = (angle * Math.PI) / 180
      face.style.transform = `perspective(500px) translateY(${radius * Math.sin(rad) - d * row}px) translateZ(${radius * (Math.cos(rad) - 1)}px) rotateX(${-angle}deg)`
      face.style.opacity = String(Math.max(0, 1 - Math.abs(d) * 0.28))
    })
  }
}

define('spec-select', Select)
