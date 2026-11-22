import { createToaster } from '@ui-sdk/web'
import '@/components/ui/toaster'
import { define, escape, Kit } from '@/lib/kit'

class ToastDemo extends Kit {
  toaster = createToaster()

  render() {
    const title = this.getAttribute('heading') ?? ''
    const description = this.getAttribute('description') ?? ''
    this.innerHTML = `<button type="button" data-kit="button" data-tone="primary">${escape(this.getAttribute('trigger'))}</button>`
    const region = document.createElement('spec-toasts') as HTMLElement & { toaster: ToastDemo['toaster'] }
    region.toaster = this.toaster
    this.append(region)
    this.firstElementChild!.addEventListener('click', () => this.toaster(title, { description, action: { label: 'Undo', onAction: () => {} } }))
  }

  disconnectedCallback() {
    this.toaster.dismiss()
  }
}

define('spec-toast-demo', ToastDemo, 'contents')
