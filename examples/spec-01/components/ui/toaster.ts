import '@ui-sdk/web/toast'
import { createToaster } from '@ui-sdk/web'
import { define, Kit } from '@/lib/kit'

const notify = createToaster()

class Toasts extends Kit {
  toaster = notify

  render() {
    this.innerHTML = `
      <ui-toast-region data-kit="toast-region">
        <template>
          <ui-toast data-kit="toast">
            <div data-kit="toast-text">
              <ui-toast-title data-kit="toast-title"></ui-toast-title>
              <ui-toast-description data-kit="toast-description"></ui-toast-description>
            </div>
            <ui-toast-action><button data-kit="button" data-tone="secondary"></button></ui-toast-action>
          </ui-toast>
        </template>
      </ui-toast-region>`
    this.querySelector('ui-toast-region')!.toaster = this.toaster
  }
}

define('spec-toasts', Toasts, 'contents')

export { notify }
