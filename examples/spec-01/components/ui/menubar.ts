import '@ui-sdk/web/menu'
import '@ui-sdk/web/menubar'
import { chosenItem } from '@/lib/chosen-item'
import { define, escape, Kit } from '@/lib/kit'

class Menubar extends Kit {
  render() {
    const menus = [...this.querySelectorAll<HTMLOptGroupElement>(':scope > optgroup')].map((group) => ({
      label: group.label,
      items: [...group.querySelectorAll('option')].map((option) => ({ label: option.label, checkable: option.hasAttribute('checkable') })),
    }))
    this.innerHTML = `
      <ui-menubar data-kit="menubar">
        ${menus
          .map(
            (menu) => `
          <ui-menu style="display: contents">
            <ui-menu-trigger><button data-kit="menubar-trigger">${escape(menu.label)}</button></ui-menu-trigger>
            <ui-menu-content data-kit="popup" align="start" side-offset="14" hidden>
              ${menu.items
                .map(({ label, checkable }) =>
                  checkable
                    ? `<ui-menu-checkbox-item value="${escape(label)}" data-kit="option" default-checked><ui-menu-item-indicator data-kit="option-check" keep-mounted>✓</ui-menu-item-indicator>${escape(label)}</ui-menu-checkbox-item>`
                    : `<ui-menu-item value="${escape(label)}" data-kit="option">${escape(label)}</ui-menu-item>`,
                )
                .join('')}
            </ui-menu-content>
          </ui-menu>`,
          )
          .join('')}
      </ui-menubar>`
    this.querySelectorAll('ui-menu-item').forEach(chosenItem)
    for (const menu of this.querySelectorAll('ui-menu')) {
      menu.addEventListener('action', (event) => this.emit('action', event.detail.value))
      menu.addEventListener('openchangecomplete', () => menu.querySelector('[data-chosen]')?.removeAttribute('data-chosen'))
    }
  }
}

define('spec-menubar', Menubar, 'contents')
