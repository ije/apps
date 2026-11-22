import '@ui-sdk/web/dialog'
import { define, escape, Kit } from '@/lib/kit'

class Dialog extends Kit {
  render() {
    const text = (name: string) => escape(this.getAttribute(name))
    this.innerHTML = `
      <ui-dialog style="display: contents">
        <ui-dialog-trigger><button data-kit="button" data-tone="secondary">${text('trigger')}</button></ui-dialog-trigger>
        <ui-dialog-backdrop data-kit="backdrop" hidden></ui-dialog-backdrop>
        <ui-dialog-content data-kit="dialog" hidden>
          <div data-kit="dialog-surface">
            <ui-dialog-title data-kit="dialog-title">${text('heading')}</ui-dialog-title>
            <ui-dialog-description data-kit="dialog-description">${text('description')}</ui-dialog-description>
            <div data-kit="actions">
              <ui-dialog-close><button data-kit="button" data-tone="secondary">Cancel</button></ui-dialog-close>
              <ui-dialog-close><button data-kit="button" data-tone="primary">${text('confirm')}</button></ui-dialog-close>
            </div>
          </div>
        </ui-dialog-content>
      </ui-dialog>`
  }
}

define('spec-dialog', Dialog, 'contents')
