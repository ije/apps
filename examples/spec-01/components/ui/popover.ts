import '@ui-sdk/web/popover'
import { define, escape, Kit } from '@/lib/kit'

class Popover extends Kit {
  render() {
    const text = (name: string) => escape(this.getAttribute(name))
    this.innerHTML = `
      <ui-popover>
        <ui-popover-trigger><button data-kit="button" data-tone="secondary">${text('trigger')}</button></ui-popover-trigger>
        <ui-popover-content data-kit="popover" side="top" align="center" side-offset="18">
          <div data-kit="popover-surface">
            <ui-popover-title data-kit="dialog-title">${text('heading')}</ui-popover-title>
            <ui-popover-description data-kit="dialog-description">${text('description')}</ui-popover-description>
            <div data-kit="actions">
              <ui-popover-close><button data-kit="button" data-tone="secondary">Cancel</button></ui-popover-close>
              <ui-popover-close><button data-kit="button" data-tone="primary">${text('confirm')}</button></ui-popover-close>
            </div>
          </div>
        </ui-popover-content>
      </ui-popover>`
  }
}

define('spec-popover', Popover, 'contents')
