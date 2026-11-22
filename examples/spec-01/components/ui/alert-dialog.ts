import '@ui-sdk/web/alert-dialog'
import '@/components/ui/icon'
import { define, escape, Kit } from '@/lib/kit'

class AlertDialog extends Kit {
  render() {
    const text = (name: string) => escape(this.getAttribute(name))
    const label = this.getAttribute('label')
    this.fill(`
      <ui-alert-dialog style="display: contents">
        <ui-alert-dialog-trigger><button data-kit="button" data-tone="secondary"${label ? ` aria-label="${escape(label)}"` : ''}><slot></slot></button></ui-alert-dialog-trigger>
        <ui-alert-dialog-backdrop data-kit="backdrop" hidden></ui-alert-dialog-backdrop>
        <ui-alert-dialog-content data-kit="dialog" hidden>
          <div data-kit="dialog-surface">
            <ui-alert-dialog-title data-kit="dialog-title">${text('heading')}</ui-alert-dialog-title>
            <ui-alert-dialog-description data-kit="dialog-description">${text('description')}</ui-alert-dialog-description>
            <div data-kit="actions">
              <ui-alert-dialog-close><button data-kit="button" data-tone="secondary"><spec-icon name="close"></spec-icon>Cancel</button></ui-alert-dialog-close>
              <ui-alert-dialog-close value="confirm"><button data-kit="button" data-tone="primary"><spec-icon name="trash"></spec-icon>${text('confirm')}</button></ui-alert-dialog-close>
            </div>
          </div>
        </ui-alert-dialog-content>
      </ui-alert-dialog>`)
    this.querySelector('[value="confirm"] button')!.addEventListener('click', () => this.emit('confirm'))
  }
}

define('spec-alert-dialog', AlertDialog, 'contents')
