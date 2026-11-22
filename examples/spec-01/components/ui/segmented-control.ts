import '@ui-sdk/web/segmented-control'
import { haptic } from '@/lib/haptics'
import { define, escape, Kit } from '@/lib/kit'

class Segmented extends Kit {
  #control?: HTMLElementTagNameMap['ui-segmented-control']

  get value() {
    return this.#control?.value ?? this.getAttribute('default-value')
  }

  set value(value: string | null) {
    if (this.#control) this.#control.value = value
    else if (value !== null) this.setAttribute('default-value', value)
  }

  render() {
    const options = this.options()
    const label = escape(this.getAttribute('label'))
    this.dataset.kit = 'field'
    this.innerHTML = `
      <label data-kit="label">${label}</label>
      <ui-segmented-control data-kit="segmented" aria-label="${label}" default-value="${escape(this.getAttribute('default-value'))}">
        <ui-segmented-control-indicator data-kit="segmented-indicator"></ui-segmented-control-indicator>
        ${options.map(({ value, label }) => `<ui-segmented-control-item data-kit="segment" data-value="${escape(value)}" value="${escape(value)}"><span data-kit="led" aria-hidden="true"></span>${escape(label)}</ui-segmented-control-item>`).join('')}
      </ui-segmented-control>`
    const root = (this.#control = this.querySelector('ui-segmented-control')!)
    const segments = root.getElementsByTagName('ui-segmented-control-item') as HTMLCollectionOf<HTMLElement>
    let drag: { x: number; id: number; moved: boolean } | null = null
    const select = (next: string) => {
      if (next === root.value) return
      root.value = next
      haptic('selection')
      this.emit('valuechange', next)
    }
    root.addEventListener('valuechange', (event) => {
      const next = event.detail.value
      if (!next) return event.preventDefault()
      if (next === root.value) return
      haptic('selection')
      this.emit('valuechange', next)
    })
    root.addEventListener('pointerdown', (event) => {
      if (event.button === 0) drag = { x: event.clientX, id: event.pointerId, moved: false }
    })
    root.addEventListener('pointermove', (event) => {
      if (!drag || drag.id !== event.pointerId) return
      if (!drag.moved) {
        if (Math.abs(event.clientX - drag.x) < 4) return
        drag.moved = true
        root.setPointerCapture(event.pointerId)
        root.dataset.dragging = ''
      }
      const x = event.clientX - root.getBoundingClientRect().left
      const all = [...segments]
      const under = all.find((segment) => x < segment.offsetLeft + segment.offsetWidth) ?? all[all.length - 1]
      const left = Math.min(all[all.length - 1].offsetLeft, Math.max(all[0].offsetLeft, x - under.offsetWidth / 2))
      root.style.setProperty('--drag-left', `${left}px`)
      if (under.dataset.value) select(under.dataset.value)
    })
    root.addEventListener('pointerup', (event) => {
      if (drag?.moved) root.releasePointerCapture(event.pointerId)
      drag = null
      delete root.dataset.dragging
      root.style.removeProperty('--drag-left')
    })
    root.addEventListener('pointercancel', () => {
      drag = null
      delete root.dataset.dragging
    })
  }
}

define('spec-segmented', Segmented)
