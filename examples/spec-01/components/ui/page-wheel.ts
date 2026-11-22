import '@ui-sdk/web/pagination'
import '@/components/ui/icon'
import { bindRepeat } from '@ui-sdk/core'
import { define, escape, Kit } from '@/lib/kit'
import { haptic } from '@/lib/haptics'

const spacing = 46
const step = 16
const radius = spacing / ((step * Math.PI) / 180)
const pitch = 4
const teeth = Math.ceil((Math.PI * radius) / pitch) + 2

class PageWheel extends Kit {
  static observedAttributes = ['page-count']
  #count = 1
  #page = 1
  #pos = 1
  #target = 1
  #center = 1
  #frame = 0
  #last = 0
  #settle = 0
  #drag: { x: number; target: number; samples: [number, number][]; moved: boolean } | null = null
  #root!: HTMLElementTagNameMap['ui-pagination']
  #wheel!: HTMLElement
  #drum!: HTMLElement
  #teeth: HTMLElement[] = []
  #keys: HTMLButtonElement[] = []
  #unbind: (() => void)[] = []

  render() {
    const label = escape(this.getAttribute('label') ?? 'Page')
    this.#count = Number(this.getAttribute('page-count') ?? 1)
    this.#page = this.#pos = this.#target = this.#center = this.#clamp(Number(this.getAttribute('page') ?? this.getAttribute('default-page') ?? 1))
    const key = (part: string, name: string) =>
      `<ui-pagination-${part}><button data-kit="button" data-tone="secondary" data-shape="round" aria-label="${name} page"><spec-icon name="${part === 'next' ? 'next' : 'prev'}"></spec-icon></button></ui-pagination-${part}>`
    const edge = (side: string) => `<div data-kit="page-wheel-teeth" data-edge="${side}" aria-hidden="true">${'<i></i>'.repeat(teeth)}</div>`
    this.innerHTML = `
      <ui-pagination data-kit="page-wheel-row" aria-label="${label}" page-count="${this.#count}" default-page="${this.#page}">
        ${key('previous', 'Previous')}
        <div data-kit="page-wheel" role="slider" tabindex="0" aria-label="${label}" aria-valuemin="1">${edge('top')}${edge('bottom')}<div data-kit="page-wheel-drum" aria-hidden="true"></div></div>
        ${key('next', 'Next')}
      </ui-pagination>`
    this.#root = this.querySelector('ui-pagination')!
    const wheel = (this.#wheel = this.querySelector('[data-kit="page-wheel"]')!)
    this.#drum = wheel.lastElementChild as HTMLElement
    this.#teeth = [...wheel.querySelectorAll('i')]
    this.#keys = [...this.querySelectorAll<HTMLButtonElement>('[data-kit="button"]')]
    this.#root.addEventListener('pagechange', (event) => {
      if (event.detail.reason !== 'imperative') this.#goTo(event.detail.value)
    })
    wheel.addEventListener('keydown', (event) => {
      const page = this.#page
      const to = ({ ArrowRight: page + 1, ArrowUp: page + 1, ArrowLeft: page - 1, ArrowDown: page - 1, Home: 1, End: this.#count, PageUp: page + 3, PageDown: page - 3 } as Record<string, number>)[event.key]
      if (to === undefined) return
      event.preventDefault()
      this.#goTo(to)
    })
    wheel.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return
      wheel.setPointerCapture(event.pointerId)
      this.#drag = { x: event.clientX, target: this.#target, samples: [], moved: false }
    })
    wheel.addEventListener('pointermove', (event) => {
      const drag = this.#drag
      if (!drag) return
      const dx = event.clientX - drag.x
      if (!drag.moved && Math.abs(dx) < 3) return
      if (!drag.moved) wheel.dataset.dragging = ''
      drag.moved = true
      const target = this.#give(drag.target - dx / spacing)
      drag.samples = [...drag.samples.slice(-4), [performance.now(), target]]
      this.#aim(target)
    })
    wheel.addEventListener('pointerup', (event) => {
      const drag = this.#drag
      this.#drag = null
      delete wheel.dataset.dragging
      if (!drag) return
      if (!drag.moved) {
        const box = wheel.getBoundingClientRect()
        return this.#goTo(Math.round(this.#target + (event.clientX - box.left - box.width / 2) / spacing))
      }
      const [a, b] = [drag.samples[0], drag.samples.at(-1)]
      const velocity = a && b && b[0] > a[0] ? (b[1] - a[1]) / (b[0] - a[0]) : 0
      this.#goTo(Math.round(this.#target + velocity * 220))
    })
    wheel.addEventListener('pointercancel', () => {
      this.#drag = null
      delete wheel.dataset.dragging
      this.#goTo(Math.round(this.#target))
    })
    wheel.addEventListener('wheel', (event) => {
      const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY
      if (!delta) return
      event.preventDefault()
      this.#aim(this.#give(this.#target + delta / spacing / 1.5))
      clearTimeout(this.#settle)
      this.#settle = window.setTimeout(() => this.#goTo(Math.round(this.#target)), 120)
    }, { passive: false })
    this.#numbers()
    this.#show()
    this.#paint()
  }

  connectedCallback() {
    super.connectedCallback()
    this.#keys.forEach((key, i) => {
      let held = false
      const reset = () => (held = false)
      const swallow = (event: MouseEvent) => {
        if (!held) return
        held = false
        event.preventDefault()
        event.stopImmediatePropagation()
      }
      key.addEventListener('pointerdown', reset, true)
      key.addEventListener('click', swallow, true)
      const unbind = bindRepeat(key, {
        delay: 380,
        repeat: 80,
        keys: [],
        onStep: ({ count, input }) => {
          if (input === 'click') return
          held = true
          if (count > 1) haptic('selection')
          this.#goTo(Math.round(this.#target) + (i ? 1 : -1))
        },
      })
      this.#unbind.push(() => {
        unbind()
        key.removeEventListener('pointerdown', reset, true)
        key.removeEventListener('click', swallow, true)
      })
    })
  }

  disconnectedCallback() {
    for (const unbind of this.#unbind.splice(0)) unbind()
    clearTimeout(this.#settle)
    cancelAnimationFrame(this.#frame)
    this.#frame = 0
  }

  update() {
    this.#count = Number(this.getAttribute('page-count') ?? 1)
    this.#root.setAttribute('page-count', String(this.#count))
    if (this.#page > this.#count) this.page = this.#count
    this.#numbers()
    this.#show()
  }

  get page() {
    return this.#page
  }

  set page(page: number) {
    if (!this.rendered) {
      this.setAttribute('page', String(page))
      return
    }
    if (page === this.#page) return
    this.#page = page
    this.#root.page = page
    this.#show()
    if (!this.#drag && this.#target !== page) this.#aim(page)
  }

  #clamp(page: number) {
    return Math.min(this.#count, Math.max(1, page))
  }

  #give(page: number) {
    return page < 1 ? 1 - (1 - page) * 0.3 : page > this.#count ? this.#count + (page - this.#count) * 0.3 : page
  }

  #goTo(next: number) {
    const page = this.#clamp(next)
    this.#aim(page)
    if (page === this.#page) return
    this.#page = page
    if (this.#root.page !== page) this.#root.page = page
    this.#show()
    this.emit('pagechange', page)
  }

  #aim(target: number) {
    this.#target = target
    if (this.#frame) return
    this.#last = performance.now()
    const tick = (now: number) => {
      const dt = Math.max(0, Math.min(64, now - this.#last))
      this.#last = now
      if (this.#drag) this.#pos = this.#target
      else this.#pos += (this.#target - this.#pos) * (1 - Math.exp(-0.014 * dt))
      if (Math.abs(this.#target - this.#pos) < 0.001) this.#pos = this.#target
      const rounded = Math.round(this.#pos)
      if (rounded !== this.#center) {
        haptic('selection')
        this.#center = rounded
        this.#numbers()
      }
      this.#paint()
      if (this.#pos === this.#target && !this.#drag) return (this.#frame = 0)
      this.#frame = requestAnimationFrame(tick)
    }
    this.#frame = requestAnimationFrame(tick)
  }

  #show() {
    const page = this.#page
    this.#wheel.ariaValueMax = String(this.#count)
    this.#wheel.ariaValueNow = String(page)
    this.#wheel.ariaValueText = `Page ${page} of ${this.#count}`
    this.#keys[0].disabled = page <= 1
    this.#keys[1].disabled = page >= this.#count
  }

  #numbers() {
    const center = this.#center
    let html = ''
    for (let n = Math.max(1, center - 6); n <= Math.min(this.#count, center + 6); n++)
      html += `<span data-n="${n}" data-kit="page-wheel-number"${n === center ? ' data-current' : ''}>${n}</span>`
    this.#drum.innerHTML = html
  }

  #paint() {
    const pos = this.#pos
    const phase = (((pos * spacing) % pitch) + pitch) % pitch
    const half = teeth / 2
    this.#teeth.forEach((tooth, i) => {
      const angle = (((i % teeth) - half) * pitch - phase) / radius
      const visible = Math.abs(angle) < Math.PI / 2
      tooth.style.transform = visible ? `translateX(${(radius * Math.sin(angle)).toFixed(2)}px) scaleX(${Math.cos(angle).toFixed(3)})` : 'scaleX(0)'
      tooth.style.opacity = visible ? (0.25 + 0.75 * Math.cos(angle)).toFixed(2) : '0'
    })
    for (const span of this.#drum.children as HTMLCollectionOf<HTMLElement>) {
      const d = Number(span.dataset.n) - pos
      const angle = Math.max(-90, Math.min(90, d * step))
      const rad = (angle * Math.PI) / 180
      span.style.transform = `translateX(${(radius * Math.sin(rad)).toFixed(2)}px) perspective(600px) rotateY(${angle.toFixed(2)}deg)`
      span.style.opacity = String(Math.max(0, 1 - Math.abs(d) * 0.2))
    }
  }
}

define('spec-page-wheel', PageWheel, 'contents')

declare global {
  interface HTMLElementTagNameMap {
    'spec-page-wheel': PageWheel
  }
}
