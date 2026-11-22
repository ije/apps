import '@ui-sdk/web/menu'
import '@/components/ui/icon'
import { chosenItem } from '@/lib/chosen-item'
import { define, escape, Kit } from '@/lib/kit'

class Menu extends Kit {
  render() {
    const items = this.options()
    this.innerHTML = `
      <ui-menu style="display: contents">
        <ui-menu-trigger><button data-kit="button" data-tone="secondary" data-shape="round" aria-label="${escape(this.getAttribute('label'))}"><spec-icon name="more"></spec-icon></button></ui-menu-trigger>
        <ui-menu-content data-kit="popup" align="start" side-offset="14" hidden>
          ${items.map(({ value, label }) => `<ui-menu-item value="${escape(value)}" data-kit="option">${escape(label)}</ui-menu-item>`).join('')}
        </ui-menu-content>
      </ui-menu>`
    const menu = this.querySelector('ui-menu')!
    this.querySelectorAll('ui-menu-item').forEach(chosenItem)
    menu.addEventListener('action', (event) => this.emit('action', event.detail.value))
    menu.addEventListener('openchangecomplete', () => menu.querySelector('[data-chosen]')?.removeAttribute('data-chosen'))
  }
}

define('spec-menu', Menu, 'contents')
