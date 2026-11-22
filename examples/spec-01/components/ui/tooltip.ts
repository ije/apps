import '@ui-sdk/web/tooltip'
import { define, escape, Kit } from '@/lib/kit'

const touch = matchMedia('(pointer: coarse)').matches

class Tooltip extends Kit {
  render() {
    const label = this.getAttribute('label') ?? ''
    this.dataset.kit = 'tooltip-wrap'
    this.fill(
      `<span data-kit="tooltip-legend" aria-hidden="true">${[...label].map((char, i) => `<span style="--i: ${i}">${escape(char)}</span>`).join('')}</span><ui-tooltip style="display: contents"${touch ? ' toggleable' : ''}><ui-tooltip-trigger><button data-kit="tooltip-trigger" aria-label="${escape(label)}"><slot></slot></button></ui-tooltip-trigger></ui-tooltip>`,
    )
    const legend = this.firstElementChild!
    const tooltip = this.querySelector('ui-tooltip')!
    let timer = 0
    const away = (event: PointerEvent) => {
      if (!this.contains(event.target as Node)) tooltip.open = false
    }
    tooltip.addEventListener('openchange', (event) => {
      const open = event.detail.value
      legend.toggleAttribute('data-open', open)
      if (!touch) return
      clearTimeout(timer)
      document.removeEventListener('pointerdown', away, true)
      if (!open) return
      timer = setTimeout(() => (tooltip.open = false), 2500)
      document.addEventListener('pointerdown', away, true)
    })
  }
}

define('spec-tooltip', Tooltip, 'inline')
