import '@ui-sdk/web/hover-card'
import { define, escape, Kit } from '@/lib/kit'

const coarse = matchMedia('(pointer: coarse)').matches

class HoverCard extends Kit {
  render() {
    const variant = this.getAttribute('variant')
    this.fill(`
      <ui-hover-card style="display: contents">
        <ui-hover-card-trigger><a data-kit="${escape(this.getAttribute('trigger-kit') ?? 'link')}" tabindex="0">${escape(this.getAttribute('trigger'))}</a></ui-hover-card-trigger>
        <ui-hover-card-content data-kit="popover" data-hover-card side="top" side-offset="16" hidden>
          <div data-kit="hover-lcd"${variant ? ` data-variant="${escape(variant)}"` : ''}><slot></slot></div>
        </ui-hover-card-content>
      </ui-hover-card>`)
    if (!coarse) return
    const card = this.querySelector('ui-hover-card')!
    const trigger = this.querySelector('a')!
    const away = (event: PointerEvent) => {
      const target = event.target as Element
      if (!trigger.contains(target) && !target.closest?.('[data-hover-card]')) card.open = false
    }
    card.addEventListener('openchange', (event) => {
      if (event.detail.reason !== 'imperative') return event.preventDefault()
      if (event.detail.value) document.addEventListener('pointerdown', away, true)
      else document.removeEventListener('pointerdown', away, true)
    })
    trigger.addEventListener('click', () => (card.open = !card.open))
  }
}

class ProfileHoverCard extends Kit {
  render() {
    const name = this.getAttribute('name') ?? ''
    const handle = escape(this.getAttribute('handle'))
    this.innerHTML = `
      <spec-hover-card trigger="${handle}">
        <span data-kit="hover-lcd-initials" aria-hidden="true" style="--i: 0">${escape(
          name
            .split(' ')
            .map((part) => part[0])
            .join(''),
        )}</span>
        <strong style="--i: 1">${escape(name)}</strong>
        <p style="--i: 2">${escape(this.getAttribute('bio'))}</p>
        <span data-kit="hover-lcd-status" style="--i: 3">${handle} · online</span>
      </spec-hover-card>`
  }
}

define('spec-hover-card', HoverCard, 'contents')
define('spec-profile-hover-card', ProfileHoverCard, 'contents')
