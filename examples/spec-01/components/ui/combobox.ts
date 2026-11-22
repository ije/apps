import '@ui-sdk/web/combobox'
import '@/components/ui/icon'
import { define, escape, Kit } from '@/lib/kit'

class Combobox extends Kit {
  render() {
    const options = this.options()
    const label = this.getAttribute('label') ?? ''
    this.innerHTML = `
      <ui-combobox>
        <ui-combobox-label data-kit="label">${escape(label)}</ui-combobox-label>
        <div data-kit="input-group">
          <ui-combobox-input><input data-kit="input" placeholder="Search…"></ui-combobox-input>
          <ui-combobox-trigger><button data-kit="input-addon" aria-label="Show ${escape(label.toLowerCase())}"><spec-icon name="chevron-down"></spec-icon></button></ui-combobox-trigger>
        </div>
        <ui-combobox-content data-kit="popup" data-screen align="start" hidden>
          <ui-combobox-empty data-kit="empty">No matches.</ui-combobox-empty>
          <ui-combobox-list>
            ${options.map(({ value, label }) => `<ui-combobox-item value="${escape(value)}" label="${escape(label)}" data-kit="option">${escape(label)}</ui-combobox-item>`).join('')}
          </ui-combobox-list>
        </ui-combobox-content>
      </ui-combobox>`
    this.querySelector('ui-combobox')!.addEventListener('valuechange', (event) => this.emit('valuechange', event.detail.value))
  }
}

define('spec-combobox', Combobox, 'contents')
