import '@ui-sdk/web/drawer'
import '@/components/ui/icon'
import { define, escape, Kit } from '@/lib/kit'

class Drawer extends Kit {
  render() {
    const text = (name: string) => escape(this.getAttribute(name))
    const content = this.children.length ? '<div data-kit="drawer-content"><slot></slot></div>' : ''
    this.fill(`
      <ui-drawer driver="pointer" style="display: contents">
        <ui-drawer-trigger><button data-kit="button" data-tone="secondary"><spec-icon name="filter"></spec-icon>${text('trigger')}</button></ui-drawer-trigger>
        <ui-drawer-backdrop data-kit="backdrop" hidden></ui-drawer-backdrop>
        <ui-drawer-content data-kit="drawer" hidden>
          <div data-kit="drawer-body">
            <span data-kit="drawer-grip" aria-hidden="true"></span>
            <header data-kit="drawer-head">
              <ui-drawer-title data-kit="dialog-title">${text('heading')}</ui-drawer-title>
              <ui-drawer-description data-kit="dialog-description">${text('description')}</ui-drawer-description>
            </header>
            ${content}
            <div data-kit="actions">
              <button data-kit="button" data-tone="secondary"><spec-icon name="close"></spec-icon>Cancel</button>
              <button data-kit="button" data-tone="primary"><spec-icon name="check"></spec-icon>Apply</button>
            </div>
          </div>
        </ui-drawer-content>
      </ui-drawer>`)
    const drawer = this.querySelector('ui-drawer')!
    // WARNING: the actions are plain buttons, not <ui-drawer-close> wrappers, so the skin's `[data-kit="actions"] > [data-kit="button"]` rules reach them.
    this.querySelector('[data-kit="actions"]')!.addEventListener('click', (event) => {
      if ((event.target as Element).closest('button')) drawer.api.close({ reason: 'close-press', event })
    })
  }
}

define('spec-drawer', Drawer, 'contents')
