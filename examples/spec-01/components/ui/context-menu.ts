import '@ui-sdk/web/menu'
import { chosenItem } from '@/lib/chosen-item'
import { define, escape, Kit } from '@/lib/kit'

const coarse = matchMedia('(pointer: coarse)').matches

class ContextMenuArea extends Kit {
  render() {
    const items = this.options()
    const label = this.getAttribute('label')
    this.innerHTML = `
      <ui-menu style="display: contents">
        <ui-menu-context-trigger data-kit="context-area">${escape(coarse ? (this.getAttribute('touch-label') ?? label) : label)}</ui-menu-context-trigger>
        <ui-menu-content data-kit="popup" side-offset="14" hidden>
          ${items.map(({ value, label }) => `<ui-menu-item value="${escape(value)}" data-kit="option">${escape(label)}</ui-menu-item>`).join('')}
        </ui-menu-content>
      </ui-menu>`
    const menu = this.querySelector('ui-menu')!
    this.querySelectorAll('ui-menu-item').forEach(chosenItem)
    menu.addEventListener('action', (event) => this.emit('action', event.detail.value))
    menu.addEventListener('openchangecomplete', () => menu.querySelector('[data-chosen]')?.removeAttribute('data-chosen'))
  }
}

define('spec-context-menu-area', ContextMenuArea, 'contents')
