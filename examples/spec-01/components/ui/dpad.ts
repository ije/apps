import '@ui-sdk/web/button'
import { bindRepeat } from '@ui-sdk/core'
import { define, Kit } from '@/lib/kit'
import { haptic } from '@/lib/haptics'

const keys: Record<string, string> = { ArrowUp: 'up', ArrowDown: 'down', ArrowLeft: 'left', ArrowRight: 'right' }

class DPad extends Kit {
  #unbind: (() => void)[] = []

  render() {
    this.dataset.kit = 'dpad'
    this.setAttribute('role', 'group')
    this.ariaLabel = this.getAttribute('label')
    this.innerHTML = `
      <span data-kit="dpad-well" aria-hidden="true"></span>
      <span data-kit="dpad-shadow" aria-hidden="true"><span data-kit="dpad-base"></span></span>
      <span data-kit="dpad-cap" aria-hidden="true"><span data-kit="dpad-dish"></span></span>
      ${['up', 'left', 'right', 'down'].map((dir) => `<ui-button><button data-kit="dpad-zone" data-dir="${dir}" aria-label="${dir[0].toUpperCase() + dir.slice(1)}"><span data-kit="dpad-arrow"></span></button></ui-button>`).join('')}`
    this.addEventListener('keydown', (event) => {
      const dir = keys[event.key]
      if (!dir) return
      event.preventDefault()
      this.emit(dir)
    })
  }

  connectedCallback() {
    super.connectedCallback()
    for (const zone of this.querySelectorAll<HTMLButtonElement>('[data-kit="dpad-zone"]'))
      this.#unbind.push(
        bindRepeat(zone, {
          delay: 380,
          repeat: 80,
          keys: [],
          onStep: ({ count }) => {
            if (count > 1) haptic('selection')
            this.emit(zone.dataset.dir!)
          },
        }),
      )
  }

  disconnectedCallback() {
    for (const unbind of this.#unbind.splice(0)) unbind()
  }
}

define('spec-dpad', DPad)
