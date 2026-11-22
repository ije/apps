import { bindRepeat } from '@ui-sdk/core'
import { define, Kit } from '@/lib/kit'
import { haptic } from '@/lib/haptics'

class ButtonGroup extends Kit {
  #unbind: (() => void)[] = []
  #resize = new ResizeObserver(() => this.style.setProperty('--cap-w', `${this.offsetWidth}px`))

  render() {
    this.dataset.kit = 'rocker'
    this.dataset.orientation = this.getAttribute('orientation') ?? 'horizontal'
    this.dataset.tone = this.getAttribute('tone') ?? 'secondary'
    this.setAttribute('role', 'group')
    this.ariaLabel = this.getAttribute('label')
    this.fill('<div data-kit="rocker-cap"><slot></slot></div>')
    this.querySelectorAll<HTMLButtonElement>('[data-kit="rocker-cap"] > button').forEach((part, i) => {
      part.type = 'button'
      part.dataset.kit = 'rocker-part'
      part.dataset.slot = 'button'
      if (!part.hasAttribute('repeat')) part.addEventListener('click', () => this.emit('press', i))
    })
  }

  connectedCallback() {
    super.connectedCallback()
    this.#resize.observe(this)
    this.querySelectorAll<HTMLButtonElement>('[data-kit="rocker-part"]').forEach((part, i) => {
      if (part.hasAttribute('repeat'))
        this.#unbind.push(
          bindRepeat(part, {
            delay: 380,
            repeat: 80,
            keys: [],
            onStep: ({ count }) => {
              if (count > 1) haptic('selection')
              this.emit('press', i)
            },
          }),
        )
    })
  }

  disconnectedCallback() {
    this.#resize.disconnect()
    for (const unbind of this.#unbind.splice(0)) unbind()
  }
}

define('spec-button-group', ButtonGroup)
