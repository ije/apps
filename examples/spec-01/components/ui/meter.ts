import '@ui-sdk/web/meter'
import { define, Kit } from '@/lib/kit'

class Meter extends Kit {
  static observedAttributes = ['label', 'value']

  render() {
    this.dataset.kit = 'meter'
    this.innerHTML = `
      <ui-meter style="display: contents">
        <div data-kit="row">
          <ui-meter-label data-kit="label"></ui-meter-label>
          <ui-meter-value data-kit="value"></ui-meter-value>
        </div>
        <ui-meter-track data-kit="meter-track"><ui-meter-indicator data-kit="meter-indicator"></ui-meter-indicator></ui-meter-track>
      </ui-meter>`
    this.update()
  }

  update() {
    this.querySelector('ui-meter-label')!.textContent = this.getAttribute('label')
    this.firstElementChild!.setAttribute('value', this.getAttribute('value') ?? '0')
  }
}

define('spec-meter', Meter)
