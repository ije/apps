import '@ui-sdk/web/checkbox'
import '@/components/ui/icon'
import { define, escape, Kit } from '@/lib/kit'

class Checkbox extends Kit {
  #checkbox?: HTMLElementTagNameMap['ui-checkbox']

  get checked() {
    return this.#checkbox ? this.#checkbox.checked === true : this.hasAttribute('default-checked')
  }

  set checked(checked: boolean) {
    if (this.#checkbox) this.#checkbox.checked = checked
    else this.toggleAttribute('default-checked', checked)
  }

  render() {
    const label = escape(this.getAttribute('label'))
    const icon = this.getAttribute('icon')
    this.dataset.kit = 'check'
    if (icon) this.dataset.icon = ''
    this.innerHTML = `<ui-checkbox data-kit="checkbox" aria-label="${label}"${this.hasAttribute('default-checked') ? ' default-checked' : ''}><ui-checkbox-indicator data-kit="checkbox-indicator" keep-mounted></ui-checkbox-indicator>${icon ? `<span data-kit="check-icon" aria-hidden="true"><spec-icon name="${escape(icon)}"></spec-icon></span>` : ''}</ui-checkbox><span data-kit="${icon ? 'visually-hidden' : 'label'}">${label}</span>`
    const checkbox = (this.#checkbox = this.querySelector('ui-checkbox')!)
    this.addEventListener('click', (event) => {
      if (!checkbox.contains(event.target as Node)) checkbox.click()
    })
    checkbox.addEventListener('checkedchange', (event) => this.emit('checkedchange', event.detail.value === true))
  }
}

define('spec-checkbox', Checkbox)
